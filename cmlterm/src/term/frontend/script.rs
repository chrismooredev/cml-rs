
use std::{collections::VecDeque, io::{self, Write}, str::FromStr, thread::JoinHandle};

use std::time::{Duration, Instant};
use futures::{FutureExt, future::Fuse, stream::{FusedStream, SplitStream}};
use thiserror::Error;
use futures::channel::mpsc::{Receiver, SendError, Sender};
use futures::{SinkExt, StreamExt};
use log::{debug, error, trace, warn};

use crate::api::{self, WaitMode};

use crate::term::common::{ConsoleDriver, ConsoleUpdate};

#[derive(Debug)]
struct RecvState<E: Send + Sync + std::fmt::Debug + std::error::Error + 'static> {
	/// Incoming commands. May be closed if the server closes.
	stdin_handler: Receiver<ScriptCmd>,
	/// Output data from the console
	from_console: futures::stream::Fuse<SplitStream<ConsoleDriver<E>>>,
	/// Data sent to the console. A TermMsg::Close should be sent when stdin_handler closes.
	to_console: Sender<String>,

	/// The currently working command (switch .stdin_handler to peekable?)
	cmd_cache: Option<ScriptCmd>,
	/// The last X bytes, used for expect strings
	last_chunk: VecDeque<u8>,
	/// The last update sent from the server
	last_update: Option<(Instant, ConsoleUpdate)>,
	/// Times we have sent commands
	commands_sent: Vec<Instant>,
	/// If we have received a data chunk from the terminal, since the last command was sent
	chunk_received_after_last_sent: bool,
	/// Used to determine if an initialization message was sent
	sent_init: bool,
}
impl<E: Send + Sync + std::fmt::Debug + std::error::Error + 'static> RecvState<E> {
	fn new(stdin_handler: Receiver<ScriptCmd>, to_console: Sender<String>, srv_recv: SplitStream<ConsoleDriver<E>>) -> RecvState<E> {
		RecvState {
			stdin_handler,
			from_console: srv_recv.fuse(),
			to_console,

			cmd_cache: None,
			last_chunk: VecDeque::with_capacity(256),
			last_update: None,
			chunk_received_after_last_sent: false,
			commands_sent: Vec::new(),
			sent_init: false,
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
			//None => if self.commands_processed == 0 { 1_000 } else { 10_000 }
			None => if self.commands_sent.len() == 0 { 1_000 } else { 10_000 }
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
				crate::update_chunkbuf(&mut self.last_chunk, &update.last_chunk);
				self.chunk_received_after_last_sent = true;
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
			//let add_newline = cmd.add_newline();
			//let mut send_command_text = false;

			// three wait "modes" - immediate, expect, prompt
			match &cmd.wait_mode {
				WaitMode::Immediate => { /* sending */},
				WaitMode::Expect(len, needle) => {
					if self.last_chunk.capacity() < *len {
						// ensure we can hold the request capacity
						self.last_chunk.reserve(len - self.last_chunk.capacity());
					}
					self.last_chunk.make_contiguous();
					let lc = self.last_chunk.as_slices().0;
					let chunk = if lc.len() >= *len {
						&lc[lc.len()-len..lc.len()]
					} else {
						lc // we are asking for more data than we are holding
					};

					// if we got more data, see if we have the expected text
					if self.chunk_received_after_last_sent && chunk.windows(needle.len()).any(|w| w == needle.as_bytes()) {
						/* sending */
						trace!("expect string found, sending");
					} else if timed_out {
						/* sending */
						trace!("expect string timed out, sending");
					} else {
						trace!("waiting for expect (cmd = {:?})", cmd);
						self.cmd_cache = Some(cmd);
						return Ok(Some(false));
					}
				},
				WaitMode::Prompt => {
					let prompt_ready = self.last_update.as_ref()
						.map(|cu| cu.1.last_prompt.as_ref())
						.flatten()
						.map(|(_, is_last)| *is_last)
						.unwrap_or(false);

					// if we sent only immediate commands, wait a bit for data to come back
					// if we 
					if self.last_update.is_none() {
						// need to initialize the prompt

						// need to initialize the console
						// send NAK FF - delete line, refresh line
						if !self.sent_init {
							debug!("initializing console...");
							self.to_console.send("\x15\x0C".to_owned()).await?;
							self.sent_init = true;
						} else if self.sent_init && timed_out {
							todo!("console init attempt #2")
						}
						self.cmd_cache = Some(cmd);
						return Ok(Some(false));
					} else if self.chunk_received_after_last_sent && prompt_ready {
						/* send it */
						trace!("prompt ready, sending");
					} else if timed_out {
						trace!("prompt timed out, sending");
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
			//self.commands_processed += 1;
			self.commands_sent.push(Instant::now());
			self.chunk_received_after_last_sent = false;
			//self.last_update = None;
			Ok(Some(true))
		} else {
			Ok(Some(false))
		}
	}
}


/// A user-driven terminal
/// Handles setting terminal title, passing over user input, translating terminal keys, etc
/// Can be designed so it does not terminate until the connection does
///
/// ### Functions:
/// * Essentially through-pipes between stdin/out and the console
/// * Sends a form feed message to "activate" the terminal on launch
/// * Maps common keyboard buttons/shortcuts to proper ANSI sequences
/// * If stdout is a tty:
///   * Update the terminal's title to the current prompt
///   * TODO: Color prompt lines/etc?
///   * TODO: function-key to emit commands to set term width/length ?
/// 
/// Contains setup data to initialize a user-terminal
pub struct ScriptedTerminal<E> {
	driver: ConsoleDriver<E>,
	//to_srv: Receiver<TermMsg>,
	meta: ScriptedMeta,
}

/// Contains the runtime information needed to drive the terminal state/IO
struct ScriptedMeta {
	/// A timeout used to inject a newline if a prompt is not found upon initializing the console
	auto_prompt_ms: u64,
	//srv_send: Sender<TermMsg>,
}

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

					//trace!("read line from stdin: {:?}", line);

					let cmd = match api::ScriptCommand::from_str(&line) {
						Ok(c) => c,
						Err(s) => api::ScriptCommand::basic(s)
					};
					//let cond = cond.map(|s| s.to_owned());
					//let cmd = &line[condlen..];

					//trace!("\\-> processed into {:?}", (&cond, &cmd));
					trace!("processed into {:?}", cmd);

					//from_stdin.send((cond, cmd.to_owned())).await?;
					from_stdin.send(cmd).await?;

					line.clear();
				}
			}))?;

		Ok((stdin_lines, jh))
	}
}

#[derive(Debug, Error)]
pub enum TtyError<E: Send + Sync + std::fmt::Debug + std::error::Error + 'static> {
	#[error("A terminal IO error occured.")]
	Io(#[from] std::io::Error),
	#[error("A console connection error occured.")]
	Connection(#[from] Box<E>),
	#[error("An error occured writing terminal title.")]
	Terminal(#[from] crossterm::ErrorKind),
	#[error("Send buffer has overfilled")]
	Buffer(#[from] SendError),
}

impl<E: Send + Sync + std::fmt::Debug + std::error::Error + 'static> ScriptedTerminal<E> {
	pub fn new(driver: ConsoleDriver<E>) -> ScriptedTerminal<E> {
		ScriptedTerminal {
			driver,
			//to_srv,
			meta: ScriptedMeta {
				auto_prompt_ms: 1000,
				//srv_send,
			},
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

		async fn fmain_loop<E: Send + Sync + std::fmt::Debug + std::error::Error + 'static>(stdin_handler: Receiver<ScriptCmd>, to_console: Sender<String>, srv_recv: SplitStream<ConsoleDriver<E>>) -> anyhow::Result<()> {
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
				const SENTINEL: &[u8] = b" --More-- ";
				if !processed_event && state.last_chunk.len() > 12 {
					let s = state.last_chunk.make_contiguous();
					let s = &s[s.len()-12..];
					if s.windows(SENTINEL.len()).any(|s| s == SENTINEL) {
						state.to_console.send(" ".to_owned()).await?;
						paged = true;
					}
				}
				if !paged { state.handle_cached_cmd(!processed_event).await?; }

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
			Result::<_, E>::Ok(())
		};

		debug!("waiting for scripted futures to complete");

		//let (handler, recv, driver): (Result<(), SendError>, Result<(), TtyError>, Result<(), WsError>) = futures::future::join3(stdin_handler, srv_recv, to_srv_driver).await;
		//handler?; recv?; driver?;
		let (main_loop_res, driver): (anyhow::Result<()>, Result<(), E>) = futures::future::join(main_loop, to_srv_driver).await;
		
		main_loop_res?; driver?;

		debug!("scripted futures have finished. joining stdin...");

		let () = stdin_jh.join().unwrap_or_else(|e| std::panic::resume_unwind(e))?;

		debug!("joined/closed stdin thread");

		Ok(())
	}
}

