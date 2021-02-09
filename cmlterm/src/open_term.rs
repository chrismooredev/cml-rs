use std::{cell::Cell, time::Instant};
use std::borrow::Cow;
use log::{error, warn, info, debug, trace};
use clap::Clap;

use ascii::{AsciiChar};
// use termion::event::{Event, Key};
use crossterm::event::{ KeyCode, KeyEvent, KeyModifiers };
use smol_str::SmolStr;
use tokio::net::TcpStream;
//use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_native_tls::native_tls as native_tls;
use tokio_native_tls::TlsConnector as TlsConnectorAsync;
use native_tls::TlsConnector;
use futures_util::{future, StreamExt};

use tokio_tungstenite::tungstenite;
use tokio_tungstenite::WebSocketStream;
use tungstenite::protocol::Message;
use tungstenite::error::Error as WsError;
use tungstenite::handshake::client::Response as WsResponse;
use tokio_native_tls::TlsStream;

type CmlResult<T> = Result<T, cml::rest::Error>;

#[derive(Clap)]
pub struct SubCmdOpen {
	uuid_or_lab: String,
	device: Option<String>,
	line: Option<u8>,
}
impl SubCmdOpen {
	pub async fn run(&self, host: &str) {
		// TODO: if necessary, request UUID from lab/device/line
		self.open_terminal(host, &self.uuid_or_lab).await;
	}

	async fn open_terminal(&self, host: &str, uuid: &str) {
		type WsSender = futures_channel::mpsc::UnboundedSender<Message>;
		//type WsReceiver = futures_channel::mpsc::UnboundedReceiver<Message>;
	
		async fn handle_terminal_input(tx: WsSender) {
			use crossterm::terminal;
			use crossterm::event::{ EventStream, Event, KeyEvent, KeyCode, KeyModifiers };
	
			// Disable line buffering, local echo, etc.
			terminal::enable_raw_mode().unwrap();
	
			let tx = &tx;
			EventStream::new()
				.take_while(move |event| {
					const CTRL_D: KeyEvent = KeyEvent {
						code: KeyCode::Char('d'),
						modifiers: KeyModifiers::CONTROL,
					};
					future::ready(match event {
						Ok(Event::Key(CTRL_D)) => {
							// print a newline since echo is disabled
							println!("\r");
	
							terminal::disable_raw_mode().unwrap();
							
							tx.unbounded_send(Message::Close(None)).unwrap();
	
							false
						},
						_ => true,
					})
				})
				.for_each(|event_res| async move {
					//debug!("(input event) {:?}", &event_res);
					
					match event_res {
						Ok(event) => match event {
							Event::Key(kevent) => match event_to_code(kevent) {
								//Ok(c) => send(&tx, c.to_string()),
								Ok(c) => tx.unbounded_send(Message::Text(c.to_string())).unwrap(),
								Err(e) => warn!("unable to convert key code to sendable sequence: {}", e),
							},
							c @ _ => warn!("unhandled terminal event: {:?}", c),
						},
						Err(e) => error!("error occured from stdin: {:?}", e)
					}
				})
				.await;
		}
		fn parse_terminal_prompt<'a>(data: &'a [u8]) -> Option<&'a str> {
			fn trim_end(s: &str) -> &str {
				//TODO: loop until no changes have been made?
				let mut s = s.trim_end();
				let removed_seqs = ["\x1B[6n"];
				for seq in &removed_seqs {
					s = s.trim_end_matches(seq);
				}
				s.trim_end()
			}
			
			// hopefully each prompt is < 32 chars
			//let data = if data.len() > 32 { &data[data.len()-32..] } else { &data };
			let s = std::str::from_utf8(data).ok()?;
			
			let s = trim_end(&s); // some devices have spaces/escape codes afterward
			s.ends_with(|c| c == '#' || c == '>' || c == '$') // IOS+Linux prompts
				.then(|| s.rfind(|c| c == '\r' || c == '\n')) // devices emit newlines differently
				.flatten()
				// get prompt from start of line to end (not including space)
				.map(|line_start| s[line_start..].trim())
		}
		async fn handle_ws_msg(ws_tx: &WsSender, message: Result<Message, WsError>) {
			use std::io::Write;
			//eprintln!("Recieved: {:?}", &message);
			
			let msg = message.unwrap();
			if let Message::Ping(d) = msg {
				ws_tx.unbounded_send(Message::Pong(d)).unwrap();
			} else if let Message::Binary(data) = msg {
				let out = std::io::stdout();
				let mut lock = out.lock();
	
				// set terminal title
				if let Some(prompt) = parse_terminal_prompt(&data) {
					//eprintln!("parsed prompt: {:?}", prompt);
					crossterm::execute!(
						lock,
						crossterm::terminal::SetTitle(prompt)
					).unwrap();
				}
	
				lock.write_all(&data).unwrap();
				lock.flush().unwrap();
			} else if let Message::Close(r) = msg {
				// log to user that server as requested closing the socket?
				//if let Some(reason) = r {
					//eprintln!("Server has closed the stream: ({}, {:?})", reason.code, reason.reason);
				//} else {
					//eprintln!("Server has closed the stream.");
				//}
				
	
				// TODO: close the stream somehow?
			} else {
				eprintln!("Unexpected websocket message type: {:?}", msg);
			}
		}
	
		let (ws_stream, _) = crate::connect_to_console(host, uuid).await.unwrap();
	
		debug!("websocket established");
	
		// create a channel to pipe stdin through
		let (server_tx, server_rx) = futures_channel::mpsc::unbounded();
	
		// stdin is block-on-read only, so spawn a new thread for it
		tokio::spawn(handle_terminal_input(server_tx.clone()));
	
		// split websocket into seperate write/read streams
		let (write, read) = ws_stream.split();
		let stdin_to_ws = server_rx
			.inspect(|msg| log_ws_message(msg, "send", false))
			.map(Ok)
			.forward(write);
		let ws_to_stdout = read
			.inspect(|msg| if let Ok(m) = msg { log_ws_message(m, "recv", false) })
			.for_each(|msg| handle_ws_msg(&server_tx, msg));
	
		// will overwrite the console title based on received messages
		set_terminal_title("CML Console").unwrap();
	
		// reprint the current line for the prompt/currently typed line + signal to user that we are ready
		server_tx.unbounded_send(Message::Text(ascii::AsciiChar::FF.to_string())).unwrap(); 
		
		futures_util::pin_mut!(stdin_to_ws, ws_to_stdout);
		future::select(stdin_to_ws, ws_to_stdout).await;
	}	
}

/**
Maps keyboard key events to a character sequence to send to a terminal.

Returns a nested result - The outer result signifies a if there is a matching code, the inner result contains the actual key codes, depending on if it can be a char or constant string.
*/
pub fn event_to_code(event: KeyEvent) -> Result<SmolStr, String> {
	//use AsciiChar::*;

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
			KeyCode::Enter => Ok('\n'),
			KeyCode::Home => Ok(AsciiChar::SOH.as_char()),
			KeyCode::End => Ok(AsciiChar::ENQ.as_char()),
			KeyCode::Delete => Ok(AsciiChar::EOT.as_char()), // remove char to right of cursor (ctrl+d ?)
			KeyCode::Esc => Ok(AsciiChar::ESC.as_char()), // ESC - Escape
			KeyCode::Backspace => Ok(AsciiChar::BackSpace.as_char()),

			// experimental based off https://www.novell.com/documentation/extend5/Docs/help/Composer/books/TelnetAppendixB.html
			KeyCode::Insert => Err(esc!("[2~")),
			KeyCode::PageUp => Err(esc!("[5~")),
			KeyCode::PageDown => Err(esc!("[6~")),
			KeyCode::BackTab => Err(esc!("OP\x09")),
			KeyCode::Null => Ok('\0'), // ctrl+spacebar ?
			KeyCode::F(1) => Err(esc!("OP")),
			KeyCode::F(2) => Err(esc!("OQ")),
			KeyCode::F(3) => Err(esc!("OR")),
			KeyCode::F(4) => Err(esc!("OS")),
			KeyCode::F(5) => Err(esc!("[15~")),
			KeyCode::F(n @ 6..=10) => Err(SmolStr::new(format!("[{}~", n+11))),
			KeyCode::F(n @ 11..=14) => Err(SmolStr::new(format!("[{}~", n+12))),
			KeyCode::F(n @ 15..=16) => Err(SmolStr::new(format!("[{}~", n+13))),
			KeyCode::F(n @ 17..=20) => Err(SmolStr::new(format!("[{}~", n+14))),
			KeyCode::F(n @ _) => Err(format!("invalid function key: 'F{}'", n))?
			
			//c @ _ => Err(format!("unexpected non-modified key: '{:?}'", c))?,
		},
		KeyEvent { code: kc, modifiers: KeyModifiers::CONTROL } => match kc {
			// ctrl key codes

			// make these "control-codes" emit as arrow keys instead
			KeyCode::Char(ch @ ('p' | 'n' | 'f' | 'b')) => Err((match ch {
				'p' => ARROW_UP,
				'n' => ARROW_DOWN,
				'f' => ARROW_RIGHT,  // right - responds with <char at new position>
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
				}
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

fn set_terminal_title(s: &str) -> Result<(), crossterm::ErrorKind> {
	crossterm::execute!(
		std::io::stdout(),
		crossterm::terminal::SetTitle(s)
	)
}
fn truncate_string<'a>(s: &'a str, len: usize) -> Cow<'a, str> {
	if s.len() > len*4 {
		Cow::from(format!("{}<...truncated...>{}", &s[..len], &s[s.len()-len..]))
	} else {
		Cow::from(s)
	}
}
fn log_ws_message(msg: &Message, id_str: &str, pings: bool) {
	match msg {
		Message::Text(s) => {
			let as_hex = hex::encode(s.as_bytes());
			trace!("{} : Text : {:?} : {:?}", id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
		},
		Message::Binary(b) => {
			let as_hex = hex::encode(&b);
			let s = String::from_utf8_lossy(&b);
			trace!("{} : Binary : {:?} : {:?}", id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
		},
		Message::Close(cf) => {
			debug!("{} : Close{}", id_str, if let Some(c) = cf { format!(" : {:?}", c) } else { "".into() });
		},
		Message::Pong(b) => {
			if pings {
				let as_hex = hex::encode(&b);
				let s = String::from_utf8_lossy(&b);
				trace!("{} : Pong : {:?} : {:?}", id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
			}
		},
		Message::Ping(b) => {
			if pings {
				let as_hex = hex::encode(&b);
				let s = String::from_utf8_lossy(&b);
				trace!("{} : Ping : {:?} : {:?}", id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
			}
		}
	}
}


