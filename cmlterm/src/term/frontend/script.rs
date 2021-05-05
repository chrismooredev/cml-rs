
use std::io::{self, Write};
use std::str::FromStr;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use futures::{FutureExt, future::Fuse, stream::{FusedStream, SplitStream}};
use thiserror::Error;
use futures::channel::mpsc::{Receiver, SendError, Sender};
use futures::{SinkExt, StreamExt};
use log::{debug, error, trace, warn};

use crate::api::{self, WaitMode};

use crate::term::BoxedDriverError;
use crate::term::common::{ConsoleDriver, ConsoleUpdate};

#[derive(Debug)]
enum InitStatus {
	None,
	Reprint,
	Newline,
	Done,
}

#[derive(Debug)]
struct RecvState{
	/// Incoming commands. May be closed if the server closes.
	stdin_handler: Receiver<ScriptCmd>,
	/// Output data from the console
	from_console: futures::stream::Fuse<SplitStream<ConsoleDriver>>,
	/// Data sent to the console. A TermMsg::Close should be sent when stdin_handler closes.
	to_console: Sender<String>,

	/// The currently working command (switch .stdin_handler to peekable?)
	cmd_cache: Option<ScriptCmd>,
	/// The last update sent from the server
	last_update: Option<(Instant, ConsoleUpdate)>,
	/// Times we have sent data
	data_sent: Vec<Instant>,
	/// Used to determine if an initialization message was sent
	sent_init: InitStatus,
}
impl RecvState {
	fn new(stdin_handler: Receiver<ScriptCmd>, to_console: Sender<String>, srv_recv: SplitStream<ConsoleDriver>) -> RecvState {
		RecvState {
			stdin_handler,
			from_console: srv_recv.fuse(),
			to_console,

			cmd_cache: None,
			last_update: None,
			data_sent: Vec::new(),
			sent_init: InitStatus::None,
		}
	}


	/// The currently active timeout. Longer in the middle of a session, or custom if a command specifies it.
	fn timeout(&self) -> Fuse<tokio::time::Sleep> {
		let cmd_timeout_ms = match &self.cmd_cache {
			Some(cmd) => cmd.timeout(),
			None => None,
		};
		let timeout_ms = match cmd_timeout_ms {
			Some(ms) => ms,
			None if self.data_sent.len() == 0 || ! matches!(self.sent_init, InitStatus::Done) => 1_000,
			None => 10_000,
		};

		tokio::time::sleep(Duration::from_millis(timeout_ms)).fuse()
	}

	/// Handles commands from the server, and closes streams as appropriate
	async fn next_event(&mut self) -> anyhow::Result<bool> {
		// wait a second for data if we haven't procesed any commands, else wait 10s for intermediate commands to go back to the prompt
		let timeout = self.timeout();
		futures::pin_mut!(timeout);

		// we don't have anything more to send, nor will we ever
		if self.cmd_cache.is_none() && self.stdin_handler.is_terminated() && !self.to_console.is_closed() {
			// tell the server we will not send any more input
			debug!("disconnecting to_console as we cannot receive more commands");
			self.to_console.disconnect();
		}

		// if we can't send commands, don't receive commands
		if self.to_console.is_closed() && !self.stdin_handler.is_terminated() {
			debug!("disconnecting stdin_handler as we cannot send more commands");
			self.stdin_handler.close();
		}

		// wait for next command, while piping output through to terminal
		// terminal could be waiting on some sort of output before passing in another command
		tokio::select! {
			// try these branches in the written order
			// specifically, handle server updates before recieving new commands over stdin
			biased;

			// get the next update from the server, overwriting previous updates if applicable
			// TODO: somehow exit with non-zero exit code if server_recv closes while commands still buffered?
			Some(res_update) = self.from_console.next() => {
				// can always receive - data should be printed to terminal either way
				let update = res_update?;
				std::io::stdout().write(&update.last_chunk)?;
				self.last_update = Some((Instant::now(), update));
			},

			// get the next item from stdin. If stdin/console has terminated, then this branch is effectively disabled.
			Some(next_cmd) = self.stdin_handler.next(), if !self.stdin_handler.is_terminated() && self.cmd_cache.is_none() => {
				self.cmd_cache = Some(next_cmd);
			},

			// timeout if we can receive data from the console
			() = timeout, if !self.from_console.is_terminated() && (self.cmd_cache.is_some() || !self.stdin_handler.is_terminated()) => {
				return Ok(false)
			},

			else => { return Ok(false) },
		};

		Ok(true)
	}

	/// Determines if the next command is ready to be sent.  
	/// If returning `None`, then it will never be ready. (Output/Input closed?)
	async fn handle_cached_cmd(&mut self, timed_out: bool) -> anyhow::Result<Option<bool>> {
		if self.to_console.is_closed() {
			return Ok(None);
		}

		// we can send data
		if self.cmd_cache.is_some() {
		//if let Some(cmd) = &self.cmd_cache {
			
			// take value out so we don't partially borrow from self
			let cmd = self.cmd_cache.take().unwrap();
			
			// three wait "modes" - immediate, expect, prompt
			match &cmd.wait_mode {
				WaitMode::Immediate => { /* sending */},
				WaitMode::Expect(len, needle) if needle.len() > *len => {
					warn!("Needle length is greater than the requested scane length. This should be considered a scripting error. Ignoring command ({:?})", &cmd);
				},
				WaitMode::Expect(_, _) if ! self.chunk_received_after_last_sent() => {
					debug!("expect is waiting without having received any data since last input sent");
				},
				WaitMode::Expect(len, needle) => {
					// SAFETY: we cannot enter this block without self.last_update being Some(_)
					let res_ref = &self.last_update.as_ref().unwrap().1.cache_ref;
					let s = res_ref.try_borrow().expect("there are no awaited borrows, and we're single threaded");
					let chunk = &s[s.len().min(*len)..];

					// if we got more data, see if we have the expected text
					if self.chunk_received_after_last_sent() && chunk.windows(needle.len()).any(|w| w == needle.as_bytes()) {
						/* sending */
						trace!("expect string found, sending");
					} else if timed_out {
						/* sending */
						debug!("expect string timed out, sending anyway");
					} else {
						trace!("waiting for expect (cmd = {:?})", cmd);
						self.cmd_cache = Some(cmd);
						return Ok(Some(false));
					}
				},
				WaitMode::Prompt if self.last_update.is_none() => {
					// need to initialize the prompt

					// need to initialize the console
					// send NAK FF - delete line, refresh line
					if let InitStatus::None = self.sent_init {
						debug!("initializing console...");
						self.to_console.send("\x15\x0C".to_owned()).await?;
						self.sent_init = InitStatus::Reprint;
					} else if matches!(self.sent_init, InitStatus::Reprint) && timed_out {
						// send newline
						debug!("init timed out, sending newline...");
						self.to_console.send("\r\n".to_owned()).await?;
						self.sent_init = InitStatus::Newline;
					} else {
						todo!("console init attempt #3")
					}
					self.cmd_cache = Some(cmd);
					return Ok(Some(false));
				},
				WaitMode::Prompt => {
					let prompt_ready = self.last_update.as_ref()
						.map(|cu| cu.1.last_prompt.as_ref())
						.flatten()
						.map(|(_, is_last)| *is_last)
						.unwrap_or(false);

					// if we sent only immediate commands, wait a bit for data to come back
					if self.chunk_received_after_last_sent() && prompt_ready {
						/* send it */
						self.sent_init = InitStatus::Done;
						trace!("prompt ready, sending");
					} else if timed_out {
						debug!("prompt timed out, sending anyway");
					} else {
						trace!("waiting for prompt");
						self.cmd_cache = Some(cmd);
						return Ok(Some(false));
					}
				}
			}

			debug!("sending command: {:?}", cmd);

			if cmd.command.len() > 0 {
				self.to_console.feed(cmd.command).await?;
			}
			if !cmd.skip_newline {
				self.to_console.feed("\r".to_owned()).await?;
			}

			self.to_console.flush().await?;
			self.data_sent.push(Instant::now());
			Ok(Some(true))
		} else {
			Ok(Some(false))
		}
	}

	
	/// If we have received a data chunk from the terminal, since the last command was sent
	fn chunk_received_after_last_sent(&self) -> bool {
		match (&self.last_update, self.data_sent.last()) {
			(Some((inst_upd8, _)), Some(inst_sent)) => {
				inst_upd8 > inst_sent
			},
			_ => false,
		}
	}
}
pub struct ScriptedTerminal {
	driver: ConsoleDriver,
	//to_srv: Receiver<TermMsg>,
	meta: ScriptedMeta,
}

/// Contains the runtime information needed to drive the terminal state/IO
struct ScriptedMeta;

//type ScriptCmd = (ScriptWaitCondition<String>, String);
type ScriptCmd = api::ScriptCommand;

#[derive(Debug, Error)]
enum ScriptedError {
	#[error("io error")]
	Io(#[from] std::io::Error),
	#[error("too many commands buffered")]
	BufferOverflow(#[from] SendError),
}

impl ScriptedMeta {

	// Creates a new thread for stdin to listen on, and returns a Receiver for each command, as well as a join handle for the thread.
	// The reciever (nd thread) will terminate at the same time as stdin, at which point the thread should be joined to discover any panics.
	fn drive_input(&self) -> io::Result<(Receiver<ScriptCmd>, JoinHandle<Result<(), ScriptedError>>)> {
		let (from_stdin, stdin_lines) = futures::channel::mpsc::channel::<ScriptCmd>(256);

		let jh = std::thread::Builder::new()
			.name("read_stdin".to_owned())
			.spawn(move || futures::executor::block_on(async {
				use std::io::BufRead;

				let stdin = std::io::stdin();
				let mut stdin = std::io::BufReader::new(stdin);
				let mut from_stdin = from_stdin;
				let mut line = String::new();

				loop {
					let read_bytes = stdin.read_line(&mut line)?;
					if read_bytes == 0 { // EOF
						// implicitly also drops `from_stdin` so the `stdin_lines` receiver closes
						debug!("stdin closed, returning from stdin thread");
						return Ok(());
					}

					let _last_line = if line.ends_with('\n') {
						line.pop().unwrap();
						true // not EOF
					} else {
						false // EOF
					};

					let cmd = match api::ScriptCommand::from_str(&line) {
						Ok(c) => c,
						Err(s) => api::ScriptCommand::basic(s)
					};

					trace!("processed into {:?}", cmd);

					from_stdin.send(cmd).await?;

					line.clear();
				}
			}))?;

		Ok((stdin_lines, jh))
	}
}

impl ScriptedTerminal {
	pub fn new(driver: ConsoleDriver) -> ScriptedTerminal {
		ScriptedTerminal {
			driver,
			//to_srv,
			meta: ScriptedMeta,
		}
	}

	/// Runs drives the console based off the current process' stdin and stdout.
	/// Recieves events from stdin in a loop, and passes it to the console.
	/// Data from the console is sent back to stdout, setting the terminal's title as appropriate.
	pub async fn run(self) -> anyhow::Result<()> {
		let ScriptedTerminal { driver, meta } = self;
		let (to_console, console_queue) = futures::channel::mpsc::channel::<String>(16);

		let (srv_send_raw, srv_recv) = driver.split::<String>();

		let (stdin_handler, stdin_jh) = meta.drive_input()?;

		// handle just prompted info for now

		async fn fmain_loop(stdin_handler: Receiver<ScriptCmd>, to_console: Sender<String>, srv_recv: SplitStream<ConsoleDriver>) -> anyhow::Result<()> {
			const PAGER_IOS: &[u8] = b" --More-- ";
			const PAGER_ASA: &[u8] = b"<--- More --->";

			let mut state = RecvState::new(stdin_handler, to_console, srv_recv);

			loop {
				let processed_event = state.next_event().await?;

				if state.to_console.is_closed() {
					if !state.stdin_handler.is_terminated() {
						debug!("Closing stdin_handler as sink to CML console has been closed");
						state.stdin_handler.close();
					}
					if state.cmd_cache.is_some() {
						debug!("console input closed while commands are still cached - discarding cmd ({:?})", state.cmd_cache.as_ref().unwrap());
						// clear out cached command - we can't send it anyway
						// TODO: debug output to stderr saying the command was discarded due to X?
						state.cmd_cache = None;
					}
				}

				let mut paged = false;

				trace!("checking for paged command output... (processed_event = {})", processed_event);
				if state.last_update.as_ref().is_some() {
					let res_ref = state.last_update.as_ref().unwrap().1.cache_ref.try_borrow();
					let s = res_ref.unwrap();
					let s = &s[s.len()-s.len().min(16)..];

					// TODO: match specifically for IOS/ASA as appropriate
					use crate::find_slice;
					let is_match = find_slice(&s, PAGER_IOS).is_some() || find_slice(&s, PAGER_ASA).is_some();
					trace!("found match for {:?}? {}", &s, is_match);
					if is_match {
						state.to_console.send(" ".to_owned()).await?;
						paged = true;
					}
				}

				if paged {
					trace!("\toutput was paged, not sending command this data chunk");
				} else {
					state.handle_cached_cmd(!processed_event).await?;
				}

				if state.to_console.is_closed() {
					debug!("to_console closed - breaking fmain_loop");
					// when closing to_console, we should have send TermMsg::Close which should alert the server to close
					
					while let Some(update) = state.from_console.next().await {
						let update = update?;
						std::io::stdout().write(&update.last_chunk)?;
					}
					break;
				}
			}

			debug!("fmain_loop has ended");

			println!(""); // add newline to usability

			anyhow::Result::Ok(())
		}

		let main_loop = fmain_loop(stdin_handler, to_console, srv_recv);


		//let srv_recv = meta.drive_output(to_console, srv_recv, stdout_lock);
		let to_srv_driver = async {
			// is there a better alternative to this?
			// (short-circuiting on error stream forwarder)
			// tried to find something like futures::stream::StreamExt::try_forward

			let mut to_srv = console_queue;
			let mut srv_send_raw = srv_send_raw;
			while let Some(msg) = to_srv.next().await {
				srv_send_raw.send(msg).await?;
			}

			srv_send_raw.close().await?;
			debug!("to_srv_driver done, srv_send_raw closed");
			Result::<_, BoxedDriverError>::Ok(())
		};

		debug!("waiting for scripted futures to complete");

		let (main_loop_res, driver): (anyhow::Result<()>, Result<(), BoxedDriverError>) = futures::future::join(main_loop, to_srv_driver).await;
		
		main_loop_res?; driver?;

		debug!("scripted futures have finished. joining stdin...");

		let () = stdin_jh.join().unwrap_or_else(|e| std::panic::resume_unwind(e))?;

		debug!("joined/closed stdin thread");

		Ok(())
	}
}

