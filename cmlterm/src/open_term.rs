use clap::Clap;
use log::{debug, error, trace, warn};
use std::borrow::Cow;

use ascii::AsciiChar;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures_util::{future, StreamExt};
use smol_str::SmolStr;

use tokio_tungstenite::tungstenite;
use tungstenite::error::Error as WsError;
use tungstenite::protocol::Message;

use cml::rest::Authenticate;
use cml::rest_types as rt;
type CmlResult<T> = Result<T, cml::rest::Error>;

#[derive(Clap)]
pub struct SubCmdOpen {
	#[clap(short, long)]
	vnc: bool,

	/// Boots the machine if necessary. If autocompleting, also shows all available devices.
	#[clap(short, long)]
	boot: bool,

	uuid_or_lab: String,
}
impl SubCmdOpen {
	pub async fn run(&self, auth: &Authenticate) -> CmlResult<()> {
		// TODO: if necessary, request UUID from lab/device/line
		let dest = &self.uuid_or_lab;
		let mut dest_uuid = None;

		let client = auth.login().await?;
		let keys = client.keys_console(true).await?;
		keys.iter()
			.for_each(|(k, _)| {
				if k == dest {
					dest_uuid = Some(k.as_str());
				}
			});
		if let None = dest_uuid {
			let split: Vec<_> = dest.split('/').collect();
			let indiv = match split.as_slice() {
				&["", lab, node] => Some((lab, node, "0")),
				&["", lab, node, line] => Some((lab, node, line)),
				_ => None,
			};
			if let Some((lab_desc, node_desc, line)) = indiv {
				let line: u64 = line.parse().expect("Unable to parse line as number");
				let lab_ids = client.labs(true).await?;
				let lab_topos: Vec<(String, rt::LabTopology)> = client.lab_topologies(&lab_ids, false).await?
					.into_iter()
					.map(|(id, topo_opt)| (id.to_string(), topo_opt.expect("Lab removed during exeuction. Rerun query")))
					.collect();
				
				let lab = lab_topos.iter()
					.find(|(id, topo)| id == lab_desc || topo.title == lab_desc);
				
				let (lab_id, lab_topo) = match lab {
					Some(l) => l,
					None => {
						eprintln!("No lab found by ID/name: {:?}", lab_desc);
						return Ok(()); // TODO: process exit code?
					}
				};

				if lab_topo.state.inactive() && ! self.boot {
					eprintln!("Lab not active, and boot flag was not passed. Exiting.");
					return Ok(());
				}

				let node = lab_topo.nodes.iter()
					.find(|node| node.id == node_desc || node.data.label == node_desc);
				
				let (node_id, node_data) = match node {
					Some(n) => (&n.id, &n.data),
					None => {
						eprintln!("No node found in specified lab by ID/name: {:?}", node_desc);
						return Ok(());
					}
				};

				if node_data.state.inactive() && ! self.boot {
					eprintln!("Node not active, and boot flag was not passed. Exiting.");
					return Ok(());
				}

				if node_data.state.inactive() && self.boot {
					// when doing so, run `GET /labs/{lab_id}/nodes/{node_id}/keys/console?line=0`
					todo!("boot up node before trying to get console key");
				}

				let k = keys.iter()
					.find(|(_, meta)| &meta.lab_id == lab_id && &meta.node_id == node_id && meta.line == line);
			
				match k {
					Some((uuid, _)) => dest_uuid = Some(uuid),
					None => {
						eprintln!("Unable to find key for specified device. Exiting.");
						return Ok(());
					}
				}
			}
		}


		if let Some(uuid) = dest_uuid {
			debug!("attemping to open console connection on {:?} to UUID {:?}", &auth.host, &uuid);
			self.open_terminal(&auth.host, &uuid).await;
		} else {
			eprintln!("Unable to find device by path or invalid UUID: {:?}", dest);
		}
		

		Ok(())
	}

	async fn open_terminal(&self, host: &str, uuid: &str) {
		use std::cell::Cell;
		type WsSender = futures_channel::mpsc::UnboundedSender<Message>;
		//type WsReceiver = futures_channel::mpsc::UnboundedReceiver<Message>;

		let received_content: Cell<bool> = Cell::new(false);
		async fn handle_terminal_input(tx: WsSender) {
			use crossterm::event::{Event, EventStream};
			use crossterm::terminal;

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
						}
						_ => true,
					})
				})
				.for_each(|event_res| async move {
					trace!("(input event) {:?}", &event_res);

					match event_res {
						Ok(event) => match event {
							Event::Key(kevent) => match event_to_code(kevent) {
								//Ok(c) => send(&tx, c.to_string()),
								Ok(c) => tx.unbounded_send(Message::Text(c.to_string())).unwrap(),
								Err(e) => warn!("unable to convert key code to sendable sequence: {}", e),
							},
							c @ _ => warn!("unhandled terminal event: {:?}", c),
						},
						Err(e) => error!("error occured from stdin: {:?}", e),
					}
				})
				.await;

			// we drop the WsSender, hopefully signalling to rx that we are done
			debug!("done processing terminal input");
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
		/// Accepts websocket messages from CML, and responds to pings, writes to stdout as necessary
		/// Also sets the terminal's title, if applicable to the message.
		async fn handle_ws_msg(ws_tx: &WsSender, message: Result<Message, WsError>, received: &Cell<bool>) {
			use std::io::Write;

			let msg = message.unwrap();
			if let Message::Ping(d) = msg {
				trace!("responding to websocket ping (message = {:?})", String::from_utf8_lossy(&d));
				ws_tx.unbounded_send(Message::Pong(d)).unwrap();
			} else if let Message::Binary(data) = msg {
				let out = std::io::stdout();
				let mut lock = out.lock();
				let mut data = data.as_slice();

				// if this is our first block of data, remove leading \r\n to prevent extra terminal line
				if received.get() == false && data.starts_with(b"\r\n"){
					data = &data[2..];
				}
				received.set(true);

				// set terminal title
				if let Some(pprompt) = parse_terminal_prompt(&data) {
					trace!("parsed prompt: {:?}", pprompt);
					crossterm::execute!(lock, crossterm::terminal::SetTitle(&pprompt)).unwrap();
				}

				lock.write_all(&data).unwrap();
				lock.flush().unwrap();
			} else if let Message::Close(_close_msg) = msg {
				// log to user that server as requested closing the socket?
				// TODO: close the stream somehow by ensuring parent doesn't take more data?
			} else {
				eprintln!("Unexpected websocket message type: {:?}", msg);
			}
		}

		/// Show a prompt to activate the terminal, if no prompt shows within 500ms of starting
		async fn show_activate_prompt(ws_tx: WsSender, received: &Cell<bool>) -> Result<(), futures::channel::mpsc::TrySendError<Message>> {
			use std::time::Duration;
			
			// reprint the current line for the prompt/currently typed line + signal to user that we are ready
			// will also prime the prompt, if possible
			ws_tx.unbounded_send(Message::Text(ascii::AsciiChar::FF.to_string())).unwrap();

			// try 3 times to activate the terminal
			let mut has_been_activated = false;
			for _ in 0..3 {
				trace!("[show_activate_prompt] sleeping for 1 secs");
				tokio::time::sleep(Duration::from_millis(1000)).await;
				trace!("[show_activate_prompt] done sleeping");

				if ! received.get() {	
					debug!("sending {:?} to activate the console", "\r");
					ws_tx.unbounded_send(Message::text("\r"))?;
				} else {
					has_been_activated = true;
					break;
				}
				
				// attempt to wait until lock is released
				trace!("[show_activate_prompt] no content sent... waiting before sending again");
			}

			if ! has_been_activated {
				eprintln!("If necessary, press F1 to activate console...");
			}
			debug!("main future 'show_activate_prompt' completed");
			Ok(())
		}

		let (ws_stream, resp) = crate::connect_to_console(host, uuid).await.unwrap();

		debug!("websocket established (HTTP status code {:?})", resp.status());

		// create a channel to pipe stdin through
		let (server_tx, server_rx) = futures_channel::mpsc::unbounded();
		let server_tx_stdin = server_tx.clone();
		let server_tx_pong = server_tx.clone();
		let server_tx_init = server_tx.clone();
		// all of our senders should be declared above
		// when all the senders are dropped/disconnected, the server send async task will exit
		std::mem::drop(server_tx);

		// stdin is block-on-read only, so spawn a new thread for it
		tokio::spawn(handle_terminal_input(server_tx_stdin));

		// will overwrite the console title based on received messages
		crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle("CML Console")).unwrap();

		// split websocket into seperate write/read streams
		let (write, read) = ws_stream.split();

		let msg_buf_to_serv = async {
			let out = server_rx
				.inspect(|msg| log_ws_message(msg, "send", false))
				.map(Ok)
				.forward(write).await;

			// this is only closed once all writers are closed
			debug!("stream 'msg_buf_to_serv' completed");
			out
		};

		let ws_content_handle = &received_content;
		let ws_to_stdout = async {
			read
				.inspect(|msg| if let Ok(m) = msg { log_ws_message(m, "recv", false) })
				.for_each(|msg| handle_ws_msg(
					&server_tx_pong, msg,
					ws_content_handle
				)).await;
			debug!("stream 'ws_to_stdout' completed");

			// make explicit so it isn't accidently broken
			// this closes this instance of the sender
			std::mem::drop(server_tx_pong);
		};

		// I'm not familiar enough with futures to know why this would be necessary for this use case.
		//futures_util::pin_mut!(msg_buf_to_serv, ws_to_stdout);

		let (s_to_ws, (), active_prompt_res) = future::join3(
			msg_buf_to_serv,
			ws_to_stdout,
			show_activate_prompt(server_tx_init, &received_content)
		).await;
		s_to_ws.expect("error sending stdin to CML console");
		active_prompt_res.expect("error attempting to activate prompt");
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
			KeyCode::Char(ch @ ('p' | 'n' | 'f' | 'b')) => Err((match ch {
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
