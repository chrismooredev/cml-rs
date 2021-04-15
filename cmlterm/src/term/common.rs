

use std::fmt::Debug;
use std::rc::Rc;
use std::future::Future;
use std::collections::VecDeque;

use futures::{future::FusedFuture, stream::{Fuse, FusedStream}};
use futures::{Sink, SinkExt, Stream, stream::SplitSink};
//use futures_util::FutureExt;
//use futures_channel::mpsc::{self, Sender, Receiver, SendError};
//use futures_channel::mpsc::{UnboundedSender, UnboundedReceiver};
//use futures_util::{future, StreamExt};
use futures::StreamExt;
use futures::channel::mpsc::Receiver;
use log::{debug, error, trace, warn};

use tokio::net::TcpStream;
use tokio_native_tls::TlsStream;
use tokio_tungstenite::{WebSocketStream, tungstenite};
use native_tls::TlsConnector;
use tokio_native_tls::native_tls;
use tokio_native_tls::TlsConnector as TlsConnectorAsync;
use tungstenite::error::{Error as WsError, Result as WsResult};
use tungstenite::protocol::Message;
use tungstenite::handshake::client::Response as WsResponse;

use crate::terminal::ConsoleCtx;


const CACHE_CAPACITY: usize = 256;

enum ConsoleError {
	ConnectionTerminated,
	ServerClosed
}

enum ConsoleEvent {
	DataMessage
}

// emit a new interface
// it will be a similar stream/sink type object

type WsSplitSink = SplitSink<WebSocketStream<TlsStream<TcpStream>>, Message>;

/// Handles asyncronously accepting input messages and eventually sending it to the WebSocket server
///
/// Should be periodically polled
#[derive(Debug)]
struct ServerHandler {
	ctx: Rc<ConsoleCtx>,
	stream: Receiver<Message>,
	sink: Option<WsSplitSink>,

	/// A buffered item in case there was no room to send
	buffered_item: Option<Message>,
}
impl ServerHandler {
	fn new(ctx: Rc<ConsoleCtx>, recv: Receiver<Message>, sink: WsSplitSink) -> ServerHandler {
		ServerHandler {
			ctx,
			stream: recv,
			sink: Some(sink),
			buffered_item: None,
		}
	}
}

impl FusedFuture for ServerHandler {
	fn is_terminated(&self) -> bool { self.sink.is_none() }
}
impl Future for ServerHandler {
	type Output = Result<(), <WsSplitSink as Sink<Message>>::Error>;
	fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		// TODO: comb through if stuff is going wrong
		// heavily modeled on futures::stream::Forward

		let ServerHandler { ctx, sink, stream, buffered_item } = &mut *self;

		//let mut st = self.stream;
		//let dynsink: &mut dyn Sink<_, Error = _> = &mut si;

		loop {
			let mut si = Pin::new(sink
				.as_mut()
				.expect("polled `ServerHandler` after completion"));
			//let st = Pin::new(stream);

			// try to send our buffered item, if possible
			if let Some(msg) = buffered_item.take() {
				match si.poll_ready_unpin(cx) {
					Poll::Ready(t) => t,
					Poll::Pending => {
						// replace it back into the option
						*buffered_item = Some(msg);
						return Poll::Pending
					}
				}?;

				ctx.log_message("send", &msg);
				si.start_send_unpin(msg)?;
			}

			// if the stream...
			//   has an item ready, buffer it
			//   is done, try to clear our sink and destroy it
			//   is pending, try to flush the sink
			
			match stream.poll_next_unpin(cx) {
				Poll::Ready(Some(item)) => {
					*buffered_item = Some(item);
				}
				Poll::Ready(None) => {
					match si.poll_close(cx) {
						Poll::Ready(t) => t,
						Poll::Pending => {
							return Poll::Pending
						}
					}?;
					self.sink = None;

					return Poll::Ready(Ok(()));
				}
				Poll::Pending => {
					match si.poll_flush_unpin(cx) {
						Poll::Ready(t) => t,
						Poll::Pending => {
							return Poll::Pending
						}
					}?;
					return Poll::Pending;
				}
			}
		}
	}
}
// impl Unpin for ServerHandler {} // implications?

/// Responsible for encapuslating and maintaining a connection to a CML device
#[derive(Debug)]
pub struct ConsoleDriver {
	ctx: Rc<ConsoleCtx>,
	ws: Fuse<WebSocketStream<TlsStream<TcpStream>>>,
	ws_close_sent: bool,
	// connection state metadata

	/// The server has sent us a `Close` message and would like to close the connection
	//server_done: bool,
	/// How many data chunks we have recieved
	received_chunks: usize,
	data_cache: VecDeque<u8>,
	
	// custom Fuse impl so we don't have to split the WebSocketStream for Fuse on the read end
	//read_done: bool,
}

impl ConsoleDriver {
	// Getters
	pub fn context(&self) -> &ConsoleCtx { &self.ctx }

	/// Initiates a websocket connection and sets up the future plumbing needed to drive the connection
	pub async fn connect(console: ConsoleCtx) -> Result<ConsoleDriver, WsError> {
		// safety: we only get ConsoleCtx structs with valid UUIDs by searching for devices
		let (ws_stream, resp) = unsafe { ConsoleDriver::connect_raw(console.node().host(), &console.uuid()).await? };

		debug!("websocket established (HTTP status code {:?})", resp.status());
		trace!("websocket headers:");
		for header in resp.headers().iter() {
			trace!("\t{:?}", header);
		}
		
		let ctx = Rc::new(console);
		//let (ws_send, ws_read) = ws_stream.split();

		Ok(ConsoleDriver {
			//uuid: ctx.uuid().to_owned(),
			ctx,
			
			// Note: the impl of WebSocketStream does not seem to error if called multiple times
			// however, it does not impl FusedStream so force it here so we can implement it ourselves
			//ws_read: ws_read.fuse(),
			//ws_send,
			ws: ws_stream.fuse(),
			ws_close_sent: false,

			// connection state metadata
			received_chunks: 0,
			data_cache: VecDeque::with_capacity(CACHE_CAPACITY),
		})
	}

	/// Attempts to connect directly to the specified UUID
	///
	/// # Safety
	/// If the UUID is malformed, the CML-side console multiplexor may crash and the instance will have to be rebooted.
	pub async unsafe fn connect_raw(host: &str, uuid: &str) -> Result<(WebSocketStream<TlsStream<TcpStream>>, WsResponse), WsError> {
		// connect to host
		let s: TcpStream = TcpStream::connect((host, 443)).await.unwrap();

		// unwrap ssl
		let connector = TlsConnector::builder()
			.danger_accept_invalid_certs(true)
			.build().unwrap();
		let connector = TlsConnectorAsync::from(connector);
		let s = connector.connect(host, s).await.unwrap();

		// TODO: validate UUID first, otherwise we may crash the VIRL2 Device mux service
		//       possible to do without authenticating and quering for console keys?

		// connect using websockets
		let endpoint = format!("wss://{}/ws/dispatch/frontend/console?uuid={}", host, uuid);
		tokio_tungstenite::client_async(endpoint, s).await
	}

	pub async fn send_text<S: Into<String>>(&mut self, msg: S) -> Result<(), WsError> {
		//self.server_tx.send(Message::text(msg)).await
		self.send(TermMsg::text(msg)).await
	}
	pub async fn send_close(&mut self) -> Result<(), WsError> {
		//self.server_tx.send(Message::Close(None)).await
		self.send(TermMsg::Close).await
	}

	/// Finds a prompt (suitable for the current device) and returns it
	fn find_prompt<'a>(&self, data: &'a [u8]) -> Option<(&'a str, bool)> {
		// TODO: properly discriminate based on device type (why we have &self)
		if let Ok(s) = std::str::from_utf8(data) {
			let prompt = s.lines()
	
				// get the last non-empty string
				.map(|s| s.trim())
				.filter_map(|s| if s.len() > 0 { Some(s) } else { None })
				.filter_map(|s| { // validate it as a proper hostname
					let node_def = &self.ctx.node().meta().node_definition;
					debug!("detecting prompt for {:?}", node_def);

					// try to validate hostnames for the specific machine types
					if node_def.is_ios() {
						// length: unlimited? (up to 99 shown on enable prompt, truncated for config/etc)

						let prompt = s.find(|c| c == '>' || c == '#').map(|i| &s[..=i])?;
						// 63 prompt len + prompt ending char + configuration mode
						if !( 2 <= prompt.len() && prompt.len() <= 63+1+(2+16) ) { return None; }
						let prompt_text = &prompt[..prompt.len()-1];

						// IOS seems to be more permissive
						if ! prompt_text.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '(' | ')')) { return None; }

						Some(prompt)
					} else if node_def.is_asa() {
						// length: 63 chars (can go higher for `(config)#` etc)
						// start/end: letter/digit
						// interior: letter/digit/hyphen

						let prompt = s.find(|c| c == '>' || c == '#').map(|i| &s[..=i])?;
						// 63 prompt len + prompt ending char + configuration mode
						if !( 2 <= prompt.len() && prompt.len() <= 63+1+(2+16) ) { return None; }
						let prompt_text = &prompt[..prompt.len()-1];

						if ! prompt_text.starts_with(|c: char| c.is_alphanumeric()) { return None; }
						if ! prompt_text.ends_with(|c: char| c.is_alphanumeric() || c == ')') { return None; }

						// ensure the middle of the prompt contains only alphanumeric, and select characters
						if ! prompt_text.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '(' | ')')) { return None; }

						Some(prompt)
					} else if node_def.is_linux() {
						// do very little processing since they can vary so widely
						let prompt = s.find(|c| c == '$' || c == '#').map(|i| &s[..=i])?;
						if prompt.len() > 100 { return None; }
						Some(prompt)
					} else { // unknown
						// unknown device - use linux "permissiveness"
						let prompt = s.find(|c| c == '>' || c == '$' || c == '#').map(|i| &s[..=i])?;
						if prompt.len() > 100 { return None; }
						Some(prompt)
					}
				})
				.last();
	
			if let Some(prompt) = prompt {
				Some(( prompt, s.trim_end().ends_with(prompt) ))
			} else {
				None
			}
		} else {
			debug!("data chunk was not UTF8, skipping prompt detection");
			None
		}
	}
}

#[derive(Debug)]
pub struct ConsoleUpdate {
	pub last_chunk: Vec<u8>,
	//last_window: Vec<u8>,
	pub last_prompt: Option<(String, bool)>,
	pub was_first: bool,
}

use std::pin::Pin;
use std::task::{Context, Poll};
impl FusedStream for ConsoleDriver {
	fn is_terminated(&self) -> bool {
		//self.ws_read.is_terminated()
		self.ws.is_terminated()
	}
}
impl Stream for ConsoleDriver {
	type Item = WsResult<ConsoleUpdate>;
	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {

		// wait until we have room to send a message

		loop {
			match self.ws.poll_ready_unpin(cx) {
				Poll::Ready(Ok(())) => {
					// there is spare capacity to send
				},
				Poll::Ready(Err(se)) => {
					// receiver has been dropped
					todo!("handle case where server_rx has been dropped? (se = {:?})", se);
				},
				Poll::Pending => {
					// no capacity - wait (in case we must respond to a ping)
					return Poll::Pending;
				},
			};

			// poll for new message
			// if message exists, handle
			//   - if non-data, return polling so user can recieve another
			//     do not return Ready(None) as that would indicate end of stream
			// otherwise return as polling
			//match Pin::new(&mut self.ws_read).poll_next(cx) {
			match Pin::new(&mut self.ws).poll_next(cx) {
				Poll::Ready(Some(msg)) => {
					// could this fail on a network condition, perhaps?
					let msg = msg.expect("no known error conditions");
					self.ctx.log_message("recv", &msg);
					match msg {
						Message::Ping(payload) => {
							if log::log_enabled!(log::Level::Trace) {
								trace!("Responding to websocket ping (message = {:?})", String::from_utf8_lossy(&payload));
							}

							self.ws.start_send_unpin(Message::Pong(payload)).expect("already verified room for message");

							// wait for a new message
						},
						Message::Close(close_msg) => {
							//self.to_websocket.unbounded_send(Message::Close(payload)).unwrap();
							debug!("Received close message");

							// would it be applicable to return Ready(None) here?
						},
						Message::Binary(odata) => {
							let was_first = self.received_chunks == 0;
							self.received_chunks += 1;

							// update rolling buffer
							crate::update_chunkbuf(&mut self.data_cache, &odata);
							self.data_cache.make_contiguous();
							let chunk = self.data_cache.as_slices().0;

							// try to find a prompt
							let prompt_data = self.find_prompt(chunk);
							trace!("detected prompt: {:?}", prompt_data);
							let last_prompt = prompt_data.map(|(s, b)| (s.to_owned(), b));

							let upd8 = ConsoleUpdate {
								last_chunk: odata,
								//last_window: chunk.to_owned(),
								last_prompt,
								was_first,
							};
							return Poll::Ready(Some(Ok(upd8)))
						},
						_ => {
							warn!("Received unexpected websocket message from server: {:?}", msg);
						},
					}
					// continue onto the next loop iteration
				},
				Poll::Ready(None) =>  {
					trace!("inner websocket returned Poll::Ready(None)");
					return Poll::Ready(None);
				},
				Poll::Pending => return Poll::Pending,
			}
		}
	}
}

#[derive(Debug, Clone)]
pub enum TermMsg {
	/// Sends data to the terminal
	Data(String),
	/// Sends a WebSocket close message
	Close,
}
impl TermMsg {
	pub fn text<S: Into<String>>(s: S) -> TermMsg {
		TermMsg::Data(s.into())
	}
}
impl<S: Into<String>> From<S> for TermMsg {
	fn from(s: S) -> TermMsg {
		TermMsg::text(s)
	}
}
impl From<TermMsg> for Message {
	fn from(t: TermMsg) -> Message {
		match t {
			TermMsg::Data(s) => Message::Text(s),
			TermMsg::Close => Message::Close(None),
		}
	}
}

impl Sink<TermMsg> for ConsoleDriver {
	type Error = WsError;
	fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		// prepare to send a value
		// producers must receive Ready(Ok(())) before calling .send

		self.ws.poll_ready_unpin(cx)
	}
	fn start_send(mut self: Pin<&mut Self>, item: TermMsg) -> Result<(), Self::Error> {
		// send data to server

		if let TermMsg::Close = item {
			debug!("ConsoleDriver sent Close WS Message manually");
		}

		self.ws.start_send_unpin(Message::from(item))
	}
	fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		// flush data to server
		
		self.ws.poll_flush_unpin(cx)
	}
	fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		// send a close message to server
		// once done, return Ready(Ok(()))
		
		if ! self.ws_close_sent {
			match self.ws.poll_ready_unpin(cx) {
				Poll::Ready(Ok(())) => {},
				p @ _ => return p,
			}
			self.ws_close_sent = true;
			debug!("ConsoleDriver sent Close WS Message on sink close");

			// send WS close message.
			// If it cannot be sent immediatey, it will return (and thus we will) a Poll::Pending
			self.ws.start_send_unpin(Message::Close(None))?;
		}

		// Close the underlying sink. If it cannot (eg; still buffering the above Close) we will return Poll::Pending
		self.ws.poll_close_unpin(cx)
	}
}
impl std::ops::Drop for ConsoleDriver {
	fn drop(&mut self) {
		if ! self.ws_close_sent {
			error!("Dropping ConsoleDriver without closing the sink. This should be considered a programming error.");
		}
	}
}

use crate::truncate_string;
trait ConsoleCtxExt {
	fn log_message(&self, direction: &str, msg: &Message);
}
impl ConsoleCtxExt for ConsoleCtx {
	fn log_message(&self, direction: &str, msg: &Message) {
		// TODO: add context prefixs to these (conditionally?)
		match msg {
			Message::Text(s) => if log::log_enabled!(log::Level::Trace) {
				let as_hex = hex::encode(s.as_bytes());
				trace!("{} : Text : {:?} : {:?}", direction, truncate_string(&as_hex, 10), truncate_string(&s, 10));
			},
			Message::Binary(b) => if log::log_enabled!(log::Level::Trace) {
				let as_hex = hex::encode(&b);
				let s = String::from_utf8_lossy(&b);
				trace!("{} : Binary ({} B) : {:?} : {:?}", direction, as_hex.len(), truncate_string(&as_hex, 10), truncate_string(&s, 10));
			},
			Message::Close(cf) => {
				debug!("{} : Close{}", direction, if let Some(c) = cf { format!(" : {:?}", c) } else { "".into() });
			},
			Message::Pong(b) => if log::log_enabled!(log::Level::Trace) {
				let as_hex = hex::encode(&b);
				let s = String::from_utf8_lossy(&b);
				trace!("{} : Pong : {:?} : {:?}", direction, truncate_string(&as_hex, 10), truncate_string(&s, 10));
			},
			Message::Ping(b) => if log::log_enabled!(log::Level::Trace) {
				let as_hex = hex::encode(&b);
				let s = String::from_utf8_lossy(&b);
				trace!("{} : Ping : {:?} : {:?}", direction, truncate_string(&as_hex, 10), truncate_string(&s, 10));
			},
		}
	}
}

