
use std::{cell::{RefCell}, io::Write};
use std::io::StdoutLock;

use std::time::Duration;
use anyhow::Context;
use futures::{FutureExt, stream::SplitStream};
use thiserror::Error;
use futures::channel::mpsc::{SendError, Sender};
use futures::{SinkExt, StreamExt};
use log::{debug, error, trace, warn};
use crossterm::terminal;
use crossterm::tty::IsTty;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, Event, EventStream};

use tokio_tungstenite::tungstenite::error::Error as WsError;
use ascii::AsciiChar;
use smol_str::SmolStr;

use super::common::{ConsoleDriver, ConsoleUpdate, TermMsg};

const CTRL_D: KeyEvent = KeyEvent {
	code: KeyCode::Char('d'),
	modifiers: KeyModifiers::CONTROL,
};

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
pub struct UserTerminal {
	driver: ConsoleDriver,
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
	fn handle_update(&self, stdout: &mut StdoutLock, update: ConsoleUpdate) -> Result<(), TtyError> {
		let ConsoleUpdate { last_chunk, last_prompt, was_first } = update;
		let mut data = last_chunk.as_slice();

		// strip double-newline that some terminals do on start
		if stdout.is_tty() && was_first && data.starts_with(b"\r\n") {
			data = &data[2..];
		}
		if let Some((prompt, _last_needle)) = last_prompt {
			if *self.last_set_prompt.borrow() != prompt {
				self.set_title(stdout, prompt)?;
			}
		}
		
		stdout.write_all(data)?;
		stdout.flush()?;

		Ok(())
	}

	/// Sets the terminal title, only if stdout is a tty
	fn set_title<O: IsTty + Write>(&self, output: &mut O, title: String) -> crossterm::Result<()> {
		if output.is_tty() {
			trace!("setting terminal title to {:?}", title);
			crossterm::execute!(output, crossterm::terminal::SetTitle(&title))?;
			*self.last_set_prompt.borrow_mut() = title
		}
		Ok(())
	}

	/// Responsible for:
	/// * attempting to initialize the console
	/// * sending stdin keystrokes to the console
	/// * sending a close message on stdin close
	async fn drive_input(&self, mut srv_send: Sender<TermMsg>) -> Result<(), SendError> {
		let mut evstream = EventStream::new();

		while let Some(event_res) = evstream.next().await {
			if let Ok(Event::Key(CTRL_D)) = event_res {
				// print a newline since echo is disabled
				println!("\r");

				terminal::disable_raw_mode().unwrap();

				srv_send.send(TermMsg::Close).await?;

				break;
			}

			trace!("(input event) {:?}", &event_res);

			let event = match event_res {
				Ok(ev) => ev,
				Err(e) => {
					error!("error occured from stdin, ignoring: {:?}", e);
					continue;
				}
			};

			match event {
				Event::Key(kevent) => match event_to_code(kevent) {
					Ok(c) => srv_send.send(TermMsg::text(c)).await?,
					Err(e) => warn!("unable to convert key code to sendable sequence: {}", e),
				},
				c @ _ => warn!("unhandled terminal event, ignored: {:?}", c),
			}
		}

		Ok(())
	}

	/// Responsible for:
	/// * Initializing the console if nothing received after X ms
	/// * Processing updates from the console driver
	async fn drive_output(&self, mut srv_send: Sender<TermMsg>, srv_recv: SplitStream<ConsoleDriver>, mut stdout_lock: StdoutLock<'_>) -> Result<(), TtyError> {
		// used to track if our console has been "initialized" - have we received data?
		let mut initialized = false;
		let slp = tokio::time::sleep(Duration::from_millis(self.auto_prompt_ms)).fuse();

		let mut srv_recv = srv_recv.fuse();
		futures::pin_mut!(slp);

		// reprint the current line (Ctrl-L) for the prompt/currently typed line + signal to user that we are ready
		// will also prime the prompt, if possible
		srv_send.send(TermMsg::text(AsciiChar::FF.as_char())).await?;

		loop {
			tokio::select! {
				// if we reach a timeout and are not initialized, try again
				() = &mut slp, if !initialized => {
					debug!("prompt not obtained after {}ms, sending newline", self.auto_prompt_ms);
					srv_send.send(TermMsg::text("\r")).await?;
				},

				// get the next update message (.next() can be safely dropped if timeout occurs)
				res_update = srv_recv.next() => match res_update {
					Some(update) => {
						trace!("received update from console");
						initialized = true;
						self.handle_update(&mut stdout_lock, update?)?;
					},
					None => {
						debug!("drive output done - received end of stream");
						break;
					}
				},
			}
		}

		debug!("drive_output done");

		Ok(())
	}
}

#[derive(Debug, Error)]
pub enum TtyError {
	#[error("A terminal IO error occured.")]
	Io(#[from] std::io::Error),
	#[error("A console connection error occured.")]
	WebSocket(#[from] WsError),
	#[error("An error occured writing terminal title.")]
	Terminal(#[from] crossterm::ErrorKind),
	#[error("Send buffer has overfilled")]
	Buffer(#[from] SendError),
}

impl UserTerminal {
	pub fn new(driver: ConsoleDriver) -> UserTerminal {
		assert!(tokio::io::stdin().is_tty(), "Attempt to initialize user terminal driver on non-stdin input");
		
		UserTerminal {
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
		let UserTerminal { driver, meta } = self;
		let (to_console, console_queue) = futures::channel::mpsc::channel::<TermMsg>(16);

		// push stdin into it
		// write output to stdout, with some buffering
		assert!(tokio::io::stdin().is_tty());
		let stdio = std::io::stdout();
		let mut stdout_lock = stdio.lock();

		terminal::enable_raw_mode().with_context(|| "enabling terminal's raw mode")?;
		let init_title = format!("CML - {}", driver.context().node().node().1);
		meta.set_title(&mut stdout_lock, init_title)?;
		
		let (srv_send_raw, srv_recv) = driver.split::<TermMsg>();

		let stdin_handler = meta.drive_input(to_console.clone())
			.inspect(|res| debug!("stdin closed: {:?}", res));
		let srv_recv = meta.drive_output(to_console, srv_recv, stdout_lock);
		let to_srv_driver = async {
			// is there a better alternative to this?
			// (short-circuiting on error stream forwarder)
			// tried to find something like futures::stream::StreamExt::try_forward

			let mut to_srv = console_queue;
			let mut srv_send_raw = srv_send_raw;
			while let Some(msg) = to_srv.next().await {
				srv_send_raw.send(msg).await?;
			}

			debug!("to_srv_driver done");
			Result::<_, WsError>::Ok(())
		};

		debug!("waiting for tty futures to complete");

		let (handler, recv, driver): (Result<(), SendError>, Result<(), TtyError>, Result<(), WsError>) = futures::future::join3(stdin_handler, srv_recv, to_srv_driver).await;
		handler?; recv?; driver?;

		debug!("tty futures have finished.");

		Ok(())
	}
}

//impl ConsoleDriver for TtyTerminal { }


/// Maps keyboard key events to a character sequence to send to a terminal.
pub fn event_to_code(event: KeyEvent) -> Result<SmolStr, String> {

	const ARROW_UP: SmolStr = esc!("[A");
	const ARROW_DOWN: SmolStr = esc!("[B");
	const ARROW_RIGHT: SmolStr = esc!("[C");
	const ARROW_LEFT: SmolStr = esc!("[D");

	/*
	special handling:
		ctrl-arrow keys are mapped to moving cursor by words at a time
		ctrl+shift+6 x -> alt+6
	*/
	
	let code: Result<char, SmolStr> = match event {
		KeyEvent { code: kc, modifiers: KeyModifiers::NONE } => match kc {
			// regular (non-ctrl) key codes
			KeyCode::Char(ch) => Ok(ch),

			KeyCode::Up => Err(ARROW_UP.into()),
			KeyCode::Down => Err(ARROW_DOWN.into()),
			KeyCode::Right => Err(ARROW_RIGHT.into()),
			KeyCode::Left => Err(ARROW_LEFT.into()),

			KeyCode::Tab => Ok('\t'),
			KeyCode::Enter => Ok('\r'),
			KeyCode::Home => Ok(AsciiChar::SOH.as_char()),
			KeyCode::End => Ok(AsciiChar::ENQ.as_char()),
			KeyCode::Delete => Ok(AsciiChar::EOT.as_char()), // remove char to right of cursor (ctrl+d ?)
			KeyCode::Esc => Ok(AsciiChar::ESC.as_char()),    // ESC - Escape
			KeyCode::Backspace => Ok(AsciiChar::BackSpace.as_char()),

			// experimental based off https://www.novell.com/documentation/extend5/Docs/help/Composer/books/TelnetAppendixB.html
			KeyCode::Insert => Err(esc!("[2~")),
			KeyCode::PageUp => Err(esc!("[5~")),
			KeyCode::PageDown => Err(esc!("[6~")),
			KeyCode::BackTab => Err(esc!("OP\x09")),
			KeyCode::Null => Ok('\0'), // ctrl+spacebar ?
			//KeyCode::F(1) => Err(esc!("OP")),
			KeyCode::F(1) => Err(SmolStr::new("\r\n")),
			KeyCode::F(2) => Err(esc!("OQ")),
			KeyCode::F(3) => Err(esc!("OR")),
			KeyCode::F(4) => Err(esc!("OS")),
			KeyCode::F(5) => Err(esc!("[15~")),
			KeyCode::F(n @ 6..=10) => Err(SmolStr::new(format!("[{}~", n + 11))),
			KeyCode::F(n @ 11..=14) => Err(SmolStr::new(format!("[{}~", n + 12))),
			KeyCode::F(n @ 15..=16) => Err(SmolStr::new(format!("[{}~", n + 13))),
			KeyCode::F(n @ 17..=20) => Err(SmolStr::new(format!("[{}~", n + 14))),
			KeyCode::F(n @ _) => Err(format!("invalid function key: 'F{}'", n))?, //c @ _ => Err(format!("unexpected non-modified key: '{:?}'", c))?,
		},
		KeyEvent { code: kc, modifiers: KeyModifiers::CONTROL } => match kc {
			// ctrl key codes

			// make these "control-codes" emit as arrow keys instead
			KeyCode::Char(ch) if matches!(ch, 'p' | 'n' | 'f' | 'b') => Err((match ch {
				'p' => ARROW_UP,
				'n' => ARROW_DOWN,
				'f' => ARROW_RIGHT, // right - responds with <char at new position>
				'b' => ARROW_LEFT,  // left - responds with <bksp>
				_ => unreachable!(),
			}).into()),

			KeyCode::Left => Err(esc!('b')),
			KeyCode::Right => Err(esc!('f')),

			// parse known control codes
			KeyCode::Char(ch) => match AsciiChar::from_ascii(ch) {
				Err(e) => { Err(format!("Attempt to use non-ascii control character: {} ({:?})", ch, e))? },
				Ok(ac) => match ascii::caret_decode(ac.to_ascii_uppercase()) {
					None => { Err(format!("No control-character for ascii character '{}'", ac))? },
					Some(ctrl_ac) => Ok(ctrl_ac.as_char()),
					/*
						references:
							experimentation
							https://etherealmind.com/cisco-ios-cli-shortcuts/
					*/
					/* IOS functions for various keys:
						^A -> Move cursor to beginning of line
						^B -> Move cursor backwards one character
						ESC B -> Move backwards one word
						^C -> Exit, Exit from config mode
						ESC C -> Make letter uppercase
						^D -> Delete current character (mapped to DEL)
						^D -> EOF, Captured by shell to close console connection
						ESC D -> Remove char to right of cursor
						^E -> Move cursor to end of line
						^F -> Move cursor forward one character
						ESC F -> Move cursor forward one word
						^G -> ?? bell
						^H -> ?? backspace key
						^I -> ?? '\t'
						^J -> ?? '\n'
						^K -> Delete line from cursor to end
						^L -> Reprint line
						ESC L -> Make letter lowercase
						^M -> ?? '\n'
						^N, Down -> Next Command
						^O -> ?? '\x0F' ??
						^P, Up -> Previous Command
						^Q -> ?? bell
						^R -> Refresh Line (Start new line, with same command)
						^S -> ?? bell
						^T -> Swap current and previous characters
						^U -> Delete whole line
						ESC U -> Make rest of word uppercase
						^V
						^W -> Delete word to left of cursor
						^X -> Delete line from cursor to start (Stores deleted text in deleted buffer)
						^Y -> Paste most recent entry in delete buffer
						ESC Y -> Paste previous entry in history buffer
						^Z -> Apply command, Exit from config mode

						Ctrl+Shift+6, x -> Break current command (Mapped to ALT+6)
					*/
				},
			},

			c @ _ => Err(format!("unexpected ctrl+key: '{:?}'", c))?,
		},
		KeyEvent { code: kc, modifiers: KeyModifiers::ALT } => match kc {
			// alt key codes

			//ctrl+shift+6, x
			// [AsciiChar::RS, 'x']
			KeyCode::Char('6') => Err(SmolStr::new("\x1Ex")),

			c @ _ => Err(format!("unexpected alt+key: '{:?}'", c))?,
		},
		KeyEvent { code: kc, modifiers: KeyModifiers::SHIFT } => match kc {
			// capital letters, etc
			KeyCode::Char(ch) => Ok(ch),

			c @ _ => Err(format!("unexpected shift key: '{:?}'", c))?,
		},
		c @ _ => Err(format!("unhandled key event: '{:?}'", c))?,
	};

	match code {
		Ok(c) => {
			let mut buf = [0u8; 4];
			let s = c.encode_utf8(&mut buf);
			Ok(SmolStr::new(s))
		},
		Err(ss) => Ok(ss),
	}
}
