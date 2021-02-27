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
	_vnc: bool,

	/// Boots the machine if necessary. If autocompleting, also shows all available devices.
	#[clap(short, long)]
	boot: bool,

	/// If accepting input over stdin, then waits for prompt before sending next command
	#[clap(short, long)]
	wait: bool,

	/// Does not affect functionality, shows lab/node IDs when auto-completing
	#[clap(short, long)]
	_ids: bool,

	/// Does not affect functionality, shows console UUIDs when auto-completing
	#[clap(short, long)]
	_uuids: bool,

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
		use tokio::sync::watch::{self, Sender, Receiver};
		type WsSender = futures_channel::mpsc::UnboundedSender<Message>;
		//type WsReceiver = futures_channel::mpsc::UnboundedReceiver<Message>;

		//let received_content: Cell<bool> = Cell::new(false);
		let (received_tx, received_rx) = watch::channel::<bool>(false);
		let (prompt_tx, prompt_rx) = watch::channel::<Option<(String, bool)>>(None);
		let prompt_rx_input = prompt_rx;

		/// Handles interactive input over a tty, or uses stdin on non-interactive inputs.
		///
		/// If using stdin, this waits until a prompt is shown from `show_activate_prompt` or gives up after 5 seconds
		async fn handle_terminal_input(tx: WsSender, stdin_should_wait: bool, term_ready: Receiver<Option<(String, bool)>>) {
			use crossterm::event::{Event, EventStream};
			use crossterm::tty::IsTty;
			use crossterm::terminal;

			if std::io::stdin().is_tty() {
				debug!("stdin: is tty");

				if stdin_should_wait {
					// TODO: detect multi-line pastes and use --wait for that?
					eprintln!("info: --wait does nothing for interactive terminals");
				}

				// Disable line buffering, local echo, etc.
				terminal::enable_raw_mode().unwrap();

				// move the sender into this scope
				let tx = tx;
				let tx = &tx;

				// EventStream will spawn a new thread for stdin, since stdin is block-on-read
				// stdin is the default for EventStream
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
			} else {
				debug!("stdin: is not tty");

				async fn handle_stdin(tx: WsSender, stdin_should_wait: bool, mut term_ready: Receiver<Option<(String, bool)>>) -> std::io::Result<()> {
					use std::time::Duration;
					use tokio::io::{ BufReader, AsyncBufReadExt };
					
					const PROMPT_TIMEOUT_FIRST: u64 = 5;
					const PROMPT_TIMEOUT_OTHER: u64 = 30;

					let mut stdin = BufReader::new(tokio::io::stdin());
					let mut linebuf = Vec::with_capacity(128);
					//let mut first_command = true;
					let mut sent_commands: usize = 0;

					while stdin.read_until(b'\n', &mut linebuf).await? != 0 {
						// TODO: allow escape character to skip the waiting mechanism
						
						// we should have flushed all previous bytes in last iteration - we should not enter this loop on EOF
						assert!(linebuf.len() > 0);
						
						// received a line
						
						if stdin_should_wait || sent_commands == 0 {
							// wait for the next prompt (with a timeout) before sending a chunk of data
							let timeout = if sent_commands == 0 { PROMPT_TIMEOUT_FIRST } else { PROMPT_TIMEOUT_OTHER };
							tokio::select! {
								_ = tokio::time::sleep(Duration::from_secs(timeout)) => {
									eprintln!("prompt timer timed out, stopping ({}s for {}, command ##{})",
										timeout,
										if sent_commands == 0 {
											format!("first prompt")
										} else {
											format!("intermediate prompt")
										},
										sent_commands
									);
								},
								_ = term_ready.changed() => {
									// surround in brackets so we don't borrow for too long

									// unwrap_or(false) so we can wait until we have a prompt - there may a command finishing
									let ready = { term_ready.borrow().as_ref().map(|(_, b)| *b).unwrap_or(false) };
									
									if ready {
										// clone the vec so we can transmit the owned data, and keep our capacity
										let s = linebuf.clone();
										tx.unbounded_send(Message::Text(String::from_utf8(s).expect("input must be valid UTF8"))).unwrap();
										linebuf.clear();
										sent_commands += 1;
									}
								}
							}
						} else {
							// clone the vec so we can transmit the owned data, and keep our capacity
							let s = linebuf.clone();
							tx.unbounded_send(Message::Text(String::from_utf8(s).expect("input must be valid UTF8"))).unwrap();
							linebuf.clear();
						}

					}

					assert!(linebuf.len() == 0, "that we flushed all buffers before ending loop");

					// EOF
					/*if linebuf.len() > 0 {
						// push remaining bytes
						tx.unbounded_send(Message::Text(String::from_utf8(linebuf).expect("input must be valid UTF8"))).unwrap();
					}*/

					debug!("stdin: reached EOF - waiting for prompt (or 30s) before closing");

					loop {
						tokio::select! {
							_ = tokio::time::sleep(std::time::Duration::from_secs(PROMPT_TIMEOUT_OTHER)) => {
								debug!("unable to find console prompt after {}s - closing connection", PROMPT_TIMEOUT_OTHER);
								break;
							},
							_ = term_ready.changed() => {
								// attempt to clear current prompt line, then start with stdin
								if let Some((_, finished)) = *term_ready.borrow() {
									if finished { break; }
								}
							},
						};
					}

					tx.unbounded_send(Message::Close(None)).unwrap();

					// we drop the WsSender, hopefully signalling to rx that we are done
					Ok(())
				}

				let res = tokio::spawn(handle_stdin(tx, stdin_should_wait, term_ready)).await.unwrap();
				res.unwrap();
			}

			debug!("done processing stdin/terminal input");
		}
		
		// returns the parsed prompt, and if the prompt is the last segment in the data (except for whitespace)
		fn parse_terminal_prompt<'a>(data: &'a [u8]) -> Option<(&'a str, bool)> {
			if let Ok(s) = std::str::from_utf8(data) {
				let prompt = s.char_indices()
					.rev() // find last if possible
					.filter(|&(_, c)| c == '\r' || c == '\n') // start going from starts of lines
					.map(|(i, _)| s[i..].trim_start())
					.filter_map(|s| {
						// IOS+Linux prompts
						let end = s.find(|c| c == '#' || c == '>' || c == '$');
						end.map(|i| &s[..i+1])
					})
					.next();
	
				trace!("detected prompt: {:?} (from data chunk {:?})", prompt, s);
				if let Some(prompt) = prompt {
					Some((
						prompt, s.trim_end().ends_with(prompt)
					))
				} else {
					None
				}
			} else {
				trace!("data chunk was not UTF8, skipping prompt detection");
				None
			}
		}


		/// Accepts websocket messages from CML, and responds to pings, writes to stdout as necessary
		/// Also sets the terminal's title, if applicable to the message.
		async fn handle_ws_msg(ws_tx: &WsSender, message: Result<Message, WsError>, received: &Sender<bool>, prompt_tx: &&mut Sender<Option<(String, bool)>>) {
			use std::io::Write;

			let msg = message.unwrap();
			if let Message::Ping(d) = msg {
				trace!("responding to websocket ping (message = {:?})", String::from_utf8_lossy(&d));
				ws_tx.unbounded_send(Message::Pong(d)).unwrap();
			} else if let Message::Binary(data) = msg {
				let out = std::io::stdout();
				let mut lock = out.lock();
				let mut data = data.as_slice();

				// set terminal title
				trace!("testing data for prompt...");
				if let Some((pprompt, pprompt_end)) = parse_terminal_prompt(&data) {
					trace!("parsed/emitting prompt: {:?}", pprompt);
					crossterm::execute!(lock, crossterm::terminal::SetTitle(&pprompt)).unwrap();
					prompt_tx.send(Some((pprompt.to_string(), pprompt_end))).expect("prompt_tx send not to fail");
				} else {
					// we can use this as a notification for if data was received or not
					prompt_tx.send(None).expect("prompt_tx send not to fail");
				}
	
				// it doesn't matter if this fails
				if data.len() > 0 { let _ = received.send(true); }

				// if this is our first block of data, remove leading \r\n to prevent extra terminal line
				// note that the prompt detection relies on leading newlines
				if *received.borrow() == false && data.starts_with(b"\r\n") {
					data = &data[2..];
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
		async fn show_activate_prompt(ws_tx: WsSender, mut received: Receiver<bool>) -> Result<(), futures::channel::mpsc::TrySendError<Message>> {
			use std::time::Duration;
			
			// reprint the current line for the prompt/currently typed line + signal to user that we are ready
			// will also prime the prompt, if possible
			ws_tx.unbounded_send(Message::Text(ascii::AsciiChar::FF.to_string())).unwrap();

			// try 3 times to activate the terminal
			let mut has_been_activated = false;
			for _ in 0..3 {
				trace!("[show_activate_prompt] sleeping for 1 secs");
				tokio::select! {
					_ = tokio::time::sleep(Duration::from_millis(1000)) => {
						trace!("[show_activate_prompt] done sleeping");
						const WAKE_STRING: &str = "\r";
						debug!("sending {:?} to activate the console", WAKE_STRING);
						ws_tx.unbounded_send(Message::text(WAKE_STRING))?;
					},
					_ = received.changed() => {
						if *received.borrow() {
							has_been_activated = true;
							break;
						}
					}
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
		trace!("websocket headers:");
		for header in resp.headers().iter() {
			trace!("\t{:?}", header);
		}
		

		// create a channel to pipe stdin through
		let (server_tx, server_rx) = futures_channel::mpsc::unbounded();
		let server_tx_stdin = server_tx.clone();
		let server_tx_pong = server_tx.clone();
		let server_tx_init = server_tx.clone();
		// all of our senders should be declared above
		// when all the senders are dropped/disconnected, the server send async task will exit
		std::mem::drop(server_tx);

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

		let ws_content_handle = &received_tx;
		let ws_to_stdout = async {
			let mut prompt_tx = prompt_tx;
			let prompt_tx = &(&mut prompt_tx);
			read
				.inspect(|msg| if let Ok(m) = msg { log_ws_message(m, "recv", false) })
				.for_each(|msg| handle_ws_msg(
					&server_tx_pong, msg,
					ws_content_handle, prompt_tx,
				)).await;
			debug!("stream 'ws_to_stdout' completed");

			// make explicit so it isn't accidently broken
			// this closes this instance of the sender
			std::mem::drop(server_tx_pong);
		};

		// I'm not familiar enough with futures to know why this would be necessary for this use case.
		//futures_util::pin_mut!(msg_buf_to_serv, ws_to_stdout);

		let (s_to_ws, (), active_prompt_res, ()) = future::join4(
			msg_buf_to_serv,
			ws_to_stdout,
			show_activate_prompt(server_tx_init, received_rx),

			// stdin is block-on-read only, but crossterm will spawn a new thread for it internally
			handle_terminal_input(server_tx_stdin, self.wait, prompt_rx_input),
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
