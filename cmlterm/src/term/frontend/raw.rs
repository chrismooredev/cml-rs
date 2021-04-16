
use std::{cell::{RefCell}, io::Write, str::Utf8Error, thread::JoinHandle};
use std::io::StdoutLock;

use anyhow::Context;
use futures::{FutureExt, stream::SplitStream};
use thiserror::Error;
use futures::channel::mpsc::{SendError, Sender};
use futures::{SinkExt, StreamExt};
use log::{debug, error, trace, warn};
use crossterm::terminal;
use crossterm::tty::IsTty;

use crate::term::common::{ConsoleDriver, ConsoleUpdate};

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
pub struct RawTerminal<E> {
	driver: ConsoleDriver<E>,
	//to_srv: Receiver<TermMsg>,
	meta: UserMeta,
}

/// Contains the runtime information needed to drive the terminal state/IO
struct UserMeta {
	/// Used to prevent writing the same prompt to the terminal multiple times over
	last_set_prompt: RefCell<String>,
	/// A timeout used to inject a newline if a prompt is not found upon initializing the console
	auto_prompt_ms: u64,
	//srv_send: Sender<TermMsg>,
}
impl UserMeta {
	fn handle_update<E: Send + Sync + std::fmt::Debug + std::error::Error>(&self, stdout: &mut StdoutLock, update: ConsoleUpdate) -> Result<(), RawError<E>> {
		let ConsoleUpdate { last_chunk, last_prompt, was_first } = update;
		let mut data = last_chunk.as_slice();
		
		stdout.write_all(data)?;
		stdout.flush()?;

		Ok(())
	}

	/// Responsible for:
	/// * attempting to initialize the console
	/// * sending stdin keystrokes to the console
	/// * sending a close message on stdin close
	fn drive_input<E: Send + Sync + std::error::Error + 'static>(&self, mut srv_send: Sender<String>) -> Result<JoinHandle<Result<(), RawError<E>>>, std::io::Error> {
		Ok(std::thread::Builder::new()
			.name("read_stdin".to_owned())
			.spawn(move || futures::executor::block_on(async {
				use std::io::Read;
				let mut buf = Vec::with_capacity(1024);
				buf.resize(1024, 0);
				let mut sin = std::io::stdin();

				loop {
					let res_filled_bytes = sin.read(&mut buf);
					let filled_bytes = match res_filled_bytes {
						Ok(0) => break,
						Ok(n) => n,
						Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
						Err(e) => return Err(e.into()),
					};
					srv_send.send(std::str::from_utf8(&buf[..filled_bytes])?.to_owned()).await?;
				}

				debug!("stdin closed");
				terminal::disable_raw_mode().unwrap();
				srv_send.close_channel();
				Result::<(), RawError<E>>::Ok(())
			}))?)
	}

	/// Responsible for:
	/// * Initializing the console if nothing received after X ms
	/// * Processing updates from the console driver
	async fn drive_output<E: Send + Sync + std::fmt::Debug + std::error::Error + 'static>(&self, mut srv_recv: SplitStream<ConsoleDriver<E>>, mut stdout_lock: StdoutLock<'_>) -> Result<(), RawError<E>> {
		
		while let Some(res_update) = srv_recv.next().await {
			let update = res_update.map_err(|e| Box::new(e))?;
			self.handle_update(&mut stdout_lock, update)?;
		}

		debug!("drive_output done");

		Ok(())
	}
}

#[derive(Debug, Error)]
pub enum RawError<E: Send + Sync + std::fmt::Debug + std::error::Error + 'static> {
	#[error("A terminal IO error occured.")]
	Io(#[from] std::io::Error),
	#[error("A console connection error occured.")]
	Connection(#[from] Box<E>),
	#[error("An error occured writing terminal title.")]
	Terminal(#[from] crossterm::ErrorKind),
	#[error("Send buffer has overfilled")]
	Buffer(#[from] SendError),
	#[error("Input was not UTF-8. This is unsupported, and a fatal error.")]
	Encoding(#[from] Utf8Error),
}

impl<E: Send + Sync + std::fmt::Debug + std::error::Error + 'static> RawTerminal<E> {
	pub fn new(driver: ConsoleDriver<E>) -> RawTerminal<E> {
		assert!(tokio::io::stdin().is_tty(), "Attempt to initialize user terminal driver on non-stdin input");
		
		RawTerminal {
			driver,
			//to_srv,
			meta: UserMeta {
				last_set_prompt: RefCell::new(String::new()),
				auto_prompt_ms: 1000,
				//srv_send,
			},
		}
	}

	/// Runs drives the console based off the current process' stdin and stdout.
	/// Recieves events from stdin in a loop, and passes it to the console.
	/// Data from the console is sent back to stdout, setting the terminal's title as appropriate.
	pub async fn run(self) -> anyhow::Result<()> {
		let RawTerminal { driver, meta } = self;
		let (to_console, console_queue) = futures::channel::mpsc::channel::<String>(16);

		// push stdin into it
		// write output to stdout, with some buffering
		let stdio = std::io::stdout();
		let mut stdout_lock = stdio.lock();

		terminal::enable_raw_mode().with_context(|| "enabling terminal's raw mode")?;

		let (srv_send_raw, srv_recv) = driver.split::<String>();

		let stdin_handler = meta.drive_input::<E>(to_console)?;
		let srv_recv = meta.drive_output(srv_recv, stdout_lock);
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

		debug!("waiting for raw futures to complete");

		let (recv, driver): (Result<(), RawError<E>>, Result<(), E>) = futures::future::join(srv_recv, to_srv_driver).await;
		recv?; driver?;

		debug!("raw futures have finished. joining stdin...");
		let () = stdin_handler.join().unwrap_or_else(|e| std::panic::resume_unwind(e))?;

		debug!("raw futures have finished.");

		Ok(())
	}
}
