
use std::io;
use std::fmt::{self, Debug};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::net::TcpStream;

use futures::{Sink, SinkExt, Stream};
use log::{debug, error};
use thiserror::Error;
use futures_io::AsyncWrite;

use async_io::Async;
use async_ssh2_lite::{AsyncChannel, AsyncSession};

use crate::term::BoxedDriverError;
use crate::terminal::ConsoleCtx;

#[derive(Error, Debug)]
pub enum SshError {
	#[error("An IO error occured.")]
	Io(#[from] io::Error),
}

pub struct SshConsoleManager {
	session: AsyncSession<TcpStream>,
}

impl SshConsoleManager {
	pub async fn new(auth: &cml::rest::Authenticate) -> Result<SshConsoleManager, io::Error> {
		async fn connect_host(host: &str) -> io::Result<Async::<TcpStream>> {
			let addrs = tokio::net::lookup_host(host.to_string() + ":22").await?;
			for addr in addrs {
				let res = async_io::Async::<TcpStream>::connect(addr).await;
				match res {
					Ok(sock) => { return Ok(sock); },
					Err(e) => { return Err(e); }
				}
			}
			return Err(io::Error::new(io::ErrorKind::NotFound, format!("could not locate {}", host)));
		}

		debug!("connecting to CML on port 22");
		let stream = connect_host(&auth.host).await?;
		let mut session = AsyncSession::new(stream, None).unwrap();
		debug!("performing SSH handshake");
		session.handshake().await.unwrap();
		debug!("authenticating");
		session.userauth_password(&auth.username, &auth.password).await.unwrap();

		Ok(SshConsoleManager {
			session,
		})
	}

	pub async fn console(&self, console: &ConsoleCtx) -> Result<SshConsole, io::Error> {
		debug!("opening channel");
		let mut channel = self.session.channel_session().await.unwrap();
		debug!("requesting pty");
		channel.request_pty("", None, None).await.unwrap();
		debug!("sending list command");
		//channel.exec(&"list").await.unwrap();
		//channel.shell().await.unwrap();
		channel.exec(&format!("open /{}/{}/{}", console.node().lab().0, console.node().node().0, console.line())).await.unwrap();
		debug!("done initializing");

		Ok(SshConsole {
			channel,
			channel_send_cache: String::new(),
			channel_closed_recv: false,

			initializing: true,
			init_buffer: Vec::new(),
		})
	}
}

pub struct SshConsole {
	channel: AsyncChannel<TcpStream>,

	channel_send_cache: String,
	channel_closed_recv: bool,
	
	// Flag enabling us to remove the initial 'Escape character' business - use EOF
	initializing: bool,
	init_buffer: Vec<u8>,
}
impl SshConsole {
	pub async fn single(console: &ConsoleCtx, auth: &cml::rest::Authenticate) -> Result<SshConsole, io::Error> {
		let manager = SshConsoleManager::new(auth).await?;
		manager.console(console).await
	}
}

impl Stream for SshConsole {
	type Item = Result<Vec<u8>, BoxedDriverError>;
	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		use futures_io::AsyncRead;

		let SshConsole { channel, channel_closed_recv, initializing, init_buffer, .. } = Pin::into_inner(self);

		while ! *channel_closed_recv {
			let mut buf = [0u8; 1024];
			match Pin::new(&mut *channel).poll_read(cx, &mut buf) {
				Poll::Ready(Ok(0)) => {
					debug!("ssh stream returned EOF");
					*channel_closed_recv = true;
					return Poll::Ready(None);
				},
				Poll::Ready(Ok(n)) => {
					if *initializing {
						debug!("processing ssh console init chunk");
						init_buffer.extend_from_slice(&buf[..n]);
						let as_str = std::str::from_utf8(init_buffer).expect("everything before init should be utf8");
						let search_str = "Escape character is '^]'.\r\n";

						match as_str.find(search_str) {
							None => { /* continue initializing */ },
							Some(i) => {
								debug!("done initializing SSH console");

								let rtn_buf = &init_buffer[i+search_str.len()..];
								*initializing = false;
								if rtn_buf.len() > 0 {
									return Poll::Ready(Some(Ok(rtn_buf.to_vec())));
								}
							}
						}
					} else {
						return Poll::Ready(Some(Ok(buf[..n].to_vec())));
					}
				},
				p @ _ => return p.map(|r| Some(r.map(|_| unreachable!()).map_err(|e| e.into()))),
			}
		}

		Poll::Ready(None)
	}
}

impl Sink<String> for SshConsole {
	type Error = BoxedDriverError;
	fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {

		let SshConsole { channel, channel_send_cache, .. } = &mut *self;

		loop {
			if channel_send_cache.len() == 0 {
				break Poll::Ready(Ok(()))
			} else {
				match Pin::new(&mut *channel).poll_write(cx, channel_send_cache.as_bytes()) {
					Poll::Ready(Ok(n)) => {
						channel_send_cache.replace_range(0..n, "");
						if channel_send_cache.len() == 0 {
							debug!("drained send cache");
						}
					},
					p @ _ => break p.map_ok(|_| unreachable!()).map_err(|e| e.into()),
				}
			}
		}
	}
	fn start_send(mut self: Pin<&mut Self>, item: String) -> Result<(), Self::Error> {
		// send data to server

		// note: ideally rust-lang would have this optimization
		// (specializing extend_one to move the allocation instead of copying if dest.len() = 0)
		// (would have to specialize and check for greater string capacity to preserve capacity guarentees?)
		// but it does not as of writing
		if self.channel_send_cache.len() == 0 {
			self.channel_send_cache = item;
		} else {
			//self.channel_send_cache.extend_one(item); // potentially more optimized (see above) but still nightly
			self.channel_send_cache.push_str(&item);
		}

		Ok(())
	}
	fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		// flush data to server

		match self.poll_ready_unpin(cx) {
			Poll::Ready(Ok(())) => {},
			p @ _ => return p,
		}

		Pin::new(&mut self.channel).poll_flush(cx).map_err(|e| e.into())
	}
	fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		// due to async-ssh2-lite, this doesn't actually close the input
		// haven't taken the time to figure out how the underlying ssh2 library works with uncompleted futures
		// eg: is it queued? how do I know when it's sent?
		//     I can't store it's future, since it takes &mut Self

		// this still seems to work for now though so I'll take a closer look if it breaks
		match Pin::new(&mut self.channel).poll_close(cx) {
			Poll::Ready(Ok(())) => {
				self.channel_closed_recv = true;
				Poll::Ready(Ok(()))
			},
			p @ _ => p.map_err(|e| e.into())
		}
	}
}

impl std::ops::Drop for SshConsole {
	fn drop(&mut self) {
		if ! self.channel_closed_recv {
			error!("Dropped SshConsole without closing the sink. This should be considered a programming error.");
		}
	}
}
impl fmt::Debug for SshConsole {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("SshConsole")
			.field("session", &"AsyncSession<TcpStream>")
			.field("channel", &"AsyncChannel<TcpStream>")
			.field("channel_send_cache", &self.channel_send_cache)
			.field("channel_closed_recv", &self.channel_closed_recv)
			.finish()
	}
}