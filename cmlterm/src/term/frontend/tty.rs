
use std::cell::RefCell;

use std::time::Duration;
use anyhow::Context;
use futures::FutureExt;
use thiserror::Error;
use futures::channel::mpsc::SendError;
use futures::{AsyncWriteExt, SinkExt, StreamExt};
use log::{debug, error, trace, warn};
use colored::Colorize;
use crossterm::terminal;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, Event, EventStream};

use ascii::AsciiChar;
use smol_str::SmolStr;

use crate::term::{BoxedDriverError, UserTUI};
use crate::term::common::{ConsoleDriver, ConsoleUpdate};

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
pub struct UserTerminal<UTUI: UserTUI> {
	tui: UTUI,
	driver: ConsoleDriver,

	term_is_raw: bool,

	/// Used to prevent writing the same prompt to the terminal multiple times over
	last_set_prompt: RefCell<String>,
	/// A timeout used to inject a newline if a prompt is not found upon initializing the console
	auto_prompt_ms: u64,
}

#[derive(Debug, Error)]
pub enum TtyError {
	#[error("A terminal IO error occured.")]
	Io(#[from] std::io::Error),
	#[error("A console connection error occured.")]
	Connection(#[from] BoxedDriverError),
	#[error("An error occured writing terminal title.")]
	Terminal(#[from] crossterm::ErrorKind),
	#[error("Send buffer has overfilled")]
	Buffer(#[from] SendError),
	#[error("Attempted to launch interactive user terminal on non-tty input")]
	NonTty,
	#[error("Attempted to launch interactive user terminal on non-stdin tty input (unimplemented)")]
	NonStdin,
}

impl<UTUI: UserTUI> UserTerminal<UTUI> {

	#[cfg(unix)]
	pub fn new(driver: ConsoleDriver, tui: UTUI) -> Result<UserTerminal<UTUI>, TtyError>
	where
		UTUI::Read: std::os::unix::io::AsRawFd
	{
		use std::os::unix::io::AsRawFd;

		if ! tui.is_tty_writer() {
			return Err(TtyError::NonTty);
		}
		let is_stdin = tui.reader().as_raw_fd() == std::os::unix::io::AsRawFd::as_raw_fd(&tokio::io::stdin());
		if ! is_stdin {
			eprintln!("{} {}", tui.reader().as_raw_fd(), std::os::unix::io::AsRawFd::as_raw_fd(&tokio::io::stdin()));
			// Is there a way to remove this requirement? Is it just crossterm blocking us?
			return Err(TtyError::NonStdin);
		}
		
		Ok(unsafe { UserTerminal::new_unsafe(driver, tui) })
	}

	#[cfg(windows)]
	pub fn new(driver: ConsoleDriver, tui: UTUI) -> Result<UserTerminal<UTUI>, TtyError>
	where
		UTUI::Read: std::os::windows::io::AsRawHandle
	{
		use std::os::windows::io::AsRawHandle;

		if ! tui.is_tty_writer() {
			return Err(TtyError::NonTty);
		}
		let is_stdin = tui.reader().as_raw_handle() == std::os::windows::io::AsRawHandle::as_raw_handle(&tokio::io::stdin());
		if ! is_stdin {
			eprintln!("{:?} {:?}", tui.reader().as_raw_handle(), std::os::windows::io::AsRawHandle::as_raw_handle(&tokio::io::stdin()));
			// Is there a way to remove this requirement? Is it just crossterm blocking us?
			return Err(TtyError::NonStdin);
		}
		
		Ok(unsafe { UserTerminal::new_unsafe(driver, tui) })
	}

	/// Creates a new UserTerminal instance.
	///
	/// ## SAFETY:
	/// Due to crossterm, this terminal driver only works when operated via stdin.
	/// So a check must be made to ensure we should be using stdin.
	pub unsafe fn new_unsafe(driver: ConsoleDriver, tui: UTUI) -> UserTerminal<UTUI> {
		UserTerminal {
			driver,
			/*meta: UserMeta {
				last_set_prompt: RefCell::new(String::new()),
				auto_prompt_ms: 1000,
				is_tty_out: tui.is_tty_writer(),
			},*/
			term_is_raw: false,
			last_set_prompt: RefCell::new(String::new()),
			auto_prompt_ms: 1000,
			tui,
		}
	}

	/// Runs drives the console based off the current process' stdin and stdout.
	/// Recieves events from stdin in a loop, and passes it to the console.
	/// Data from the console is sent back to stdout, setting the terminal's title as appropriate.
	pub async fn run(mut self) -> anyhow::Result<()> {

		terminal::enable_raw_mode().with_context(|| "enabling terminal's raw mode")?;
		self.term_is_raw = true;

		let init_title = format!("{}", self.driver.context().node().node().1);
		self.set_title(init_title).await?;
		
		let res = self.drive_terminals().await;

		// make double-sure we aren't raw anymore
		let _ = terminal::disable_raw_mode()
			.map_err(|_| warn!("error disabling terminal raw mode."));

		let () = res?;

		Ok(())
	}
	async fn drive_terminals(&mut self) -> Result<(), TtyError> {
		let mut evstream = EventStream::new();

		// used to track if our console has been "initialized" - have we received data?
		let mut initialized = false;

		let slp = tokio::time::sleep(Duration::from_millis(self.auto_prompt_ms)).fuse();

		//let mut srv_recv = srv_recv.fuse();
		futures::pin_mut!(slp);

		// reprint the current line (Ctrl-L) for the prompt/currently typed line + signal to user that we are ready
		// will also prime the prompt, if possible
		self.driver.send(AsciiChar::FF.to_string()).await?;
		// TODO: try_send incase the sink was closed

		let mut closed_recv = false;
		let mut closed_tui_input = false;

		loop {
			tokio::select! {
				// if we reach a timeout and are not initialized, try again
				() = &mut slp, if !initialized => {
					debug!("prompt not obtained after {}ms, sending newline", self.auto_prompt_ms);
					self.driver.send("\r".to_owned()).await?;
					// TODO: try_send incase the sink was closed
				},

				// console data
				res_update = self.driver.next(), if !closed_recv => {
					match res_update {
						Some(update) => {
							trace!("received update from console");
							initialized = true;
							self.handle_update(update?).await?;
						},
						None => {
							closed_recv = true;
							debug!("drive output done - received end of stream");
						}
					}
				},

				// user input
				event_res_opt = evstream.next(), if !closed_tui_input => {
					let event_res = match event_res_opt {
						Some(er) => er,
						None => {
							closed_tui_input = true;
							debug!("stdin/event stream has closed");
							continue;
						}
					};

					if let Ok(Event::Key(CTRL_D)) = event_res {
						// print a newline since echo is disabled
						self.tui.write(b"\r\n").await?;
		
						terminal::disable_raw_mode()?;
						
						closed_tui_input = true;
						debug!("closing stdin manually from ctrl-d");
						self.driver.close().await?;
						self.tui.close().await?;
						continue;
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
							//Ok(c) => srv_send.send(c.to_string()).await?,
							Ok(c) => self.driver.send(c.to_string()).await?,
							Err(e) => warn!("unable to convert key code to sendable sequence: {}", e),
						},
						c @ _ => warn!("unhandled terminal event, ignored: {:?}", c),
					}
				},

				else => {
					debug!("tokio select loop has ran out of tasks; breaking");
					break;
				}
			}
		}

		debug!("drive_terminals done");

		Ok(())
	}

	async fn handle_update(&mut self, update: ConsoleUpdate) -> Result<(), TtyError> {
		let ConsoleUpdate { last_chunk, last_prompt, was_first, cache_ref } = update;
		let mut data = last_chunk;

		// strip double-newline that some terminals do on start, if we're outputting to a user
		while self.tui.is_tty_writer() && was_first && data.starts_with(b"\r\n") {
			data = data[2..].to_vec();
		}
		if let Some((prompt, last_needle)) = last_prompt {
			// if terminal coloring is enabled, color the prompt
			if colored::control::SHOULD_COLORIZE.should_colorize() && last_needle {
				let cache = cache_ref.try_borrow().expect("there to be nothing borrowing this");
				assert!(cache.ends_with(&data));
				match (String::from_utf8(data.clone()), std::str::from_utf8(&cache)) {
					(Ok(chunk), Ok(cache)) => {
						assert!(cache.trim_end().ends_with(&prompt), "data cache doesn't contain prompt despite reporting so");

						// find the last instance of our prompt
						match chunk.rfind(&prompt) {
							Some(i) => {
								// last chunk contains whole prompt
								let prompt = &chunk[i..];

								// truncate to before colored section, then reappend colored string
								data.truncate(data.len() - prompt.len());
								data.extend_from_slice(format!("{}", prompt.red()).as_bytes());
								
								//@ TODO: if on IOS, color hostname/config prompts differently?
								// self.driver.context().node().meta()
							},
							None => {
								// last chunk contains partial prompt
								assert!(prompt.ends_with(chunk.trim_end()), "chunk containing partial prompt does not end with prompt despite reporting so");

								// find number of chars we have to reach into the cache for

								// prompt_length = prompt.len() with optional added trailing whitespace
								let prompt_index = cache.rfind(&prompt).unwrap();

								data.push(AsciiChar::CarriageReturn.as_byte());

								// append a colored prompt to chunk
								data.extend_from_slice(cache[prompt_index..].red().to_string().as_bytes());
							}
						}
					},
					_ => {
						// one or both not UTF8, skip prompt colorization
					},
				}
			}

			// set terminal title
			if *self.last_set_prompt.borrow() != prompt {
				self.set_title(prompt).await?;
			}
		}
		
		self.tui.write_all(&data).await?;
		self.tui.flush().await?;

		Ok(())
	}

	async fn set_title(&mut self, title: String) -> crossterm::Result<()> {
		use crossterm::Command;

		if self.tui.is_tty_writer() {
			if *self.last_set_prompt.borrow() == title {
				return Ok(());
			}

			trace!("setting terminal title to {:?}", title);

			// async version of:
			// crossterm::execute!(&mut as_file, crossterm::terminal::SetTitle(&title))?;
			// (also does not support windows < 10)

			let cmd_title = crossterm::terminal::SetTitle(&title);
			let mut str_title = String::new();
			cmd_title.write_ansi(&mut str_title).unwrap();

			self.tui.write(str_title.as_bytes()).await?;

			*self.last_set_prompt.borrow_mut() = title;
		}
		Ok(())
	}
}

impl<UTUI: UserTUI> std::ops::Drop for UserTerminal<UTUI> {
	fn drop(&mut self) {
		if self.term_is_raw {
			self.term_is_raw = false;
			if let Err(e) = terminal::disable_raw_mode().with_context(|| "disabling terminal's raw mode") {
				eprintln!("{:?}", e);
			}
		}
	}
}

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
