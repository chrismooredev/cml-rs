use std::borrow::Cow;
use clap::Clap;
use futures::future::Either;
use futures::stream::{SplitSink, SplitStream};
use futures_channel::mpsc::UnboundedReceiver;
use futures_util::{future, StreamExt};
use log::{debug, error, trace, warn};
use tokio_native_tls::TlsStream;

use ascii::AsciiChar;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use smol_str::SmolStr;

use tokio_tungstenite::{WebSocketStream, tungstenite};
use tungstenite::error::Error as WsError;
use tungstenite::protocol::Message;

use cml::rest::Authenticate;
use cml::rest_types as rt;
type CmlResult<T> = Result<T, cml::rest::Error>;

const CTRL_D: KeyEvent = KeyEvent {
	code: KeyCode::Char('d'),
	modifiers: KeyModifiers::CONTROL,
};

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
		
		debug!("closed terminal");

		Ok(())
	}

	async fn open_terminal(&self, host: &str, uuid: &str) {
		let (ws_stream, resp) = crate::connect_to_console(host, uuid).await.unwrap();

		debug!("websocket established (HTTP status code {:?})", resp.status());
		trace!("websocket headers:");
		for header in resp.headers().iter() {
			trace!("\t{:?}", header);
		}
		
		TerminalHandler::runner(&self, ws_stream).await;
	}

}

use crossterm::tty::IsTty;
use std::io::Write;
use futures_util::FutureExt;
use tokio::{net::TcpStream, sync::watch::{self, Sender, Receiver}};
use futures_channel::mpsc::UnboundedSender;
type WsSender = futures_channel::mpsc::UnboundedSender<Message>;

#[derive(Debug, Clone)]
struct TerminalHandler/*<O: IsTty + Write>*/ {
	//output: O,
	to_websocket: futures_channel::mpsc::UnboundedSender<Message>,
	wait_for_prompt: bool,

	received_first: Receiver<bool>,
	last_prompt: Receiver<Option<(String, bool)>>,
	last_data: Receiver<Option<Vec<u8>>>,
}
impl std::ops::Drop for TerminalHandler {
	fn drop(&mut self) {
		debug!("dropping TerminalHandler");
	}
}
impl/*<O: IsTty + Write>*/ TerminalHandler {
	async fn runner(inst: &SubCmdOpen, ws_stream: WebSocketStream<TlsStream<TcpStream>>) {

		// create handler
		// inserts receivers/terminal handles into it

		// split websocket into seperate write/read streams
		let (write, read) = ws_stream.split();

		let (server_tx, server_rx) = futures_channel::mpsc::unbounded();

		let (received_tx, received_rx) = watch::channel::<bool>(false);
		let (last_data_tx, last_data_rx) = watch::channel::<Option<Vec<u8>>>(None);
		let (prompt_tx, prompt_rx) = watch::channel::<Option<(String, bool)>>(None);
		//let prompt_rx_input = prompt_rx;

		let handler = TerminalHandler {
			//output: todo!(),
			to_websocket: server_tx,
			wait_for_prompt: inst.wait,
			received_first: received_rx,
			last_prompt: prompt_rx,
			last_data: last_data_rx,
		};

		// will overwrite the console title based on received messages
		TerminalHandler::set_terminal_title(&mut std::io::stdout(), "CML Console").unwrap();

		// start up the four "main loops"

		let forward_to_server = TerminalHandler::handle_to_server(server_rx, write)
			.then(async move |r| { debug!("handler completed: forward_to_server"); r });
		let show_activate_prompt = handler.clone().handle_prompt_activation()
			.then(async move |r| { debug!("handler completed: show_activate_prompt"); r }); // done
		let handle_terminal_input = handler.clone().handle_terminal_input()
			.then(async move |r| { debug!("handler completed: handle_terminal_input"); r }); // done
		let handle_from_server = handler.clone().handle_from_server(read, prompt_tx, last_data_tx, received_tx)
			.then(async move |r| { debug!("handler completed: handle_from_server"); r }); // done

		std::mem::drop(handler);

		// wait for them all to finish
		// note that 'forward_to_server' depends on all the others to finish first
		let (s_to_ws, active_prompt_res, (), ()) = future::join4(
			forward_to_server, show_activate_prompt,
			handle_terminal_input, handle_from_server,
		).await;
		s_to_ws.expect("error sending stdin to CML console");
		active_prompt_res.expect("error attempting to activate prompt");

		()

		// launch pieces that require the senders
		//todo!();
	}
	
	/// Handles interactive input over a tty, or uses stdin on non-interactive inputs.
	///
	/// If using stdin, this waits until a prompt is shown from `show_activate_prompt` or gives up after 5 seconds
	async fn handle_terminal_input(self, /*tx: WsSender, stdin_should_wait: bool, term_ready: Receiver<Option<(String, bool)>>, data_ready: Receiver<Option<Vec<u8>>>*/) {
		use crossterm::event::{Event, EventStream};
		use crossterm::terminal;

		if std::io::stdin().is_tty() {
			debug!("stdin: is tty");

			if self.wait_for_prompt {
				// TODO: detect multi-line pastes and use --wait for that?
				eprintln!("info: --wait does nothing for interactive terminals");
			}

			// Disable line buffering, local echo, etc.
			terminal::enable_raw_mode().unwrap();

			// move the sender into this scope
			let tx = self.to_websocket.clone();
			let tx = &tx;

			// EventStream will spawn a new thread for stdin, since stdin is block-on-read
			// stdin is the default for EventStream
			EventStream::new()
				.take_while(move |event| {
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
				.for_each(async move |event_res| {
					trace!("(input event) {:?}", &event_res);

					match event_res {
						Ok(event) => match event {
							Event::Key(kevent) => match event_to_code(kevent) {
								Ok(c) => tx.unbounded_send(Message::text(c)).unwrap(),
								Err(e) => warn!("unable to convert key code to sendable sequence: {}", e),
							},
							c @ _ => warn!("unhandled terminal event, ignored: {:?}", c),
						},
						Err(e) => error!("error occured from stdin, ignoring: {:?}", e),
					}
				})
				.await;
		} else {
			debug!("stdin: is not tty");

			let moved_self = self.clone();
			//let moved_self = &moved_self;
			// since stdin is a blocking read, spawn it on a thread that may block
			tokio::task::spawn_blocking(async move || moved_self.handle_stdin_script().await); //.await.unwrap().await.unwrap();
		}
	}
	
	async fn handle_to_server(server_rx: UnboundedReceiver<Message>, write: SplitSink<WebSocketStream<TlsStream<TcpStream>>, Message>) -> tungstenite::Result<()> {
		let out = server_rx
			.inspect(|msg| log_ws_message(msg, "send", false))
			.map(Ok)
			.forward(write).await;

		// this is only closed once all writers are closed
		out
	}

	async fn handle_from_server(self, read: SplitStream<WebSocketStream<TlsStream<TcpStream>>>, prompt_tx: Sender<Option<(String, bool)>>, last_data_tx: Sender<Option<Vec<u8>>>, received_tx: Sender<bool>) {
		let mut read = read;

		while let Some(msg) = read.next().await {
			if let Ok(m) = &msg { log_ws_message(m, "recv", false) }

			self.handle_ws_msg(&self.to_websocket, msg,
				&prompt_tx, &last_data_tx, &received_tx,
			).await;
		}
	}

	/// Accepts websocket messages from CML, and responds to pings, writes to stdout as necessary
	/// Also sets the terminal's title, if applicable to the message.
	async fn handle_ws_msg(&self, ws_tx: &UnboundedSender<Message>, message: Result<Message, WsError>, prompt_tx: &Sender<Option<(String, bool)>>, last_data_tx: &Sender<Option<Vec<u8>>>, received_tx: &Sender<bool>) {
		let msg = message.unwrap();
		if let Message::Ping(d) = msg {
			trace!("responding to websocket ping (message = {:?})", String::from_utf8_lossy(&d));
			ws_tx.unbounded_send(Message::Pong(d)).unwrap();
		} else if let Message::Binary(odata) = msg {
			let out = std::io::stdout();
			let mut lock = out.lock();
			let mut data = odata.as_slice();

			// set terminal title
			let prompt_data = parse_terminal_prompt(data);
			trace!("detected prompt: {:?}", prompt_data);
			if let Some((pprompt, pprompt_end)) = prompt_data {
				TerminalHandler::set_terminal_title(&mut lock, pprompt).unwrap();
				prompt_tx.send(Some((pprompt.to_string(), pprompt_end))).expect("prompt_tx send not to fail");
			} else {
				// we can use this as a notification for if data was received or not
				prompt_tx.send(None).expect("prompt_tx send not to fail");
			}

			// it doesn't matter if this fails
			if data.len() > 0 { let _ = received_tx.send(true); }

			// if this is our first block of data, remove leading \r\n to prevent extra terminal line
			// note that the prompt detection relies on leading newlines
			if *received_tx.borrow() == false && data.starts_with(b"\r\n") {
				data = &data[2..];
			}

			lock.write_all(&data).unwrap();

			// shouldn't /really/ matter if this fails
			let _ = last_data_tx.send(Some(odata));

			lock.flush().unwrap();
		} else if let Message::Close(_close_msg) = msg {
			// log to user that server as requested closing the socket?
			// TODO: close the stream somehow by ensuring parent doesn't take more data?
		} else {
			eprintln!("Unexpected websocket message type: {:?}", msg);
		}
	}


	async fn handle_stdin_script(&self) -> std::io::Result<()> {
		use std::time::Duration;
		use tokio::io::{ BufReader, AsyncBufReadExt };
		
		const PROMPT_TIMEOUT_FIRST: u64 = 5;
		const PROMPT_TIMEOUT_OTHER: u64 = 5;

		let tx: WsSender = self.to_websocket.clone();
		let stdin_should_wait = self.wait_for_prompt;
		let mut term_ready = self.last_prompt.clone();
		let mut data_ready = self.last_data.clone();

		/// Wait for the next prompt, or wait until we have not recieved data in `timeout` seconds.
		/// Returns `true` on a found prompt.
		async fn wait_for_prompt(timeout: u64, mut ready_condition: Either<&mut Receiver<Option<(String, bool)>>, (&str, &mut Receiver<Option<Vec<u8>>>)>) -> bool {
			// this can be problematic of the server sends the prompt in seperate data chunks

			// keep looping until `term_ready` gives us a prompt, or we haven't received data for `timeout` seconds
			// otherwise we could try once, and have term_ready give us `None`, signaling received non-prompt data
			loop {
				match &mut ready_condition {
					Either::Left(term_ready) => {
						tokio::select! {
							_ = tokio::time::sleep(Duration::from_secs(timeout)) => {
								return false;
							},
							_ = term_ready.changed() => {
								// wait until we have a prompt
								if let Some((_, finished)) = *term_ready.borrow() {
									if finished { return true; }
								}
							},
						}
					},
					Either::Right((expected, data_ready)) => {
						tokio::select! {
							_ = tokio::time::sleep(Duration::from_secs(timeout)) => {
								return false;
							},
							_ = data_ready.changed() => {
								// wait until we have a prompt
								if let Some(chunk) = data_ready.borrow().as_ref() {
									if chunk.windows(expected.len())
										.find(|s| s == &expected.as_bytes())
										.is_some() { return true; }
									//if chunk.contains(expected.as_bytes()) { return true; }
								}
							},
						}
					},
				}
			}
		}

		let mut stdin_lines = BufReader::new(tokio::io::stdin()).lines();
		let mut sent_commands: usize = 0;
		let mut timed_out = false;

		// note that we only check for a timeout after having something to send

		while let Some(mut line) = stdin_lines.next_line().await? {
			// TODO: allow escape character to skip the waiting mechanism (telnet, etc)
			trace!("stdin (piped): accepted line {:?}", line);

			// if we were told to wait or this is our first command (and this line isn't "escaped")
			if (stdin_should_wait || sent_commands == 0) && !line.starts_with('~') {
				// wait for the next prompt (with a timeout) before sending a chunk of data
				let timeout = if sent_commands == 0 { PROMPT_TIMEOUT_FIRST } else { PROMPT_TIMEOUT_OTHER };

				let alternative_expect: Option<&str> = if line.starts_with('`') {
					line.char_indices()
						.filter(|(i, _)| *i != 0) // not the first one
						.filter(|(_, c)| *c == '`') // we are a grave
						.filter(|(i, _)| ! line[..*i].ends_with('\\')) // previous is not a backslash
						.map(|(i, _)| &line[1..i]) // string between the two graves
						.next()
				} else {
					None
				};

				let ready_condition = alternative_expect
					.map(|s| Either::Right((s, &mut data_ready)))
					.unwrap_or_else(|| Either::Left(&mut term_ready));
				
				match ready_condition {
					Either::Left(_) => trace!("stdin: waiting for timeout or prompt"),
					Either::Right((ex, _)) => trace!("stdin: waiting for timeout or expect string {:?}", ex),
				};

				if ! wait_for_prompt(timeout, ready_condition).await {
					eprintln!("prompt timer timed out, stopping (waited {}s for command #{})", timeout, sent_commands);
					timed_out = true;
					break;
				}

				if let Some(e) = alternative_expect {
					// remove the expected text from the data to send
					let expect_length = 1+e.len()+1;
					line.replace_range(0..expect_length, "");
				}
			}

			// remove escape character, or escaped escape character
			if line.starts_with("\\~") || line.starts_with('~') {
				line.remove(0);
			}
			line.push('\r');
			tx.unbounded_send(Message::Text(line)).unwrap();
			sent_commands += 1;
		};

		debug!("stdin: reached EOF - waiting for prompt (or 30s) before closing");
		if !timed_out && !wait_for_prompt(PROMPT_TIMEOUT_OTHER, Either::Left(&mut term_ready)).await {
			// we did not time out, and we did not find a prompt after this command
			eprintln!("unable to find console prompt after {}s - closing connection", PROMPT_TIMEOUT_OTHER);
		}

		tx.unbounded_send(Message::Close(None)).unwrap();

		// we drop the WsSender, hopefully signalling to rx that we are done
		Ok(())
	}


	/// Show a prompt to activate the terminal, if no prompt shows within 500ms of starting
	async fn handle_prompt_activation(self) -> Result<(), futures::channel::mpsc::TrySendError<Message>> {
		use std::time::Duration;
		let ws_tx = self.to_websocket.clone();
		let mut received = self.received_first.clone();
		
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
		Ok(())
	}

	/// Sets the current terminal's title, if we are interactive
	fn set_terminal_title<O: IsTty + Write>(output: &mut O, title: &str) -> crossterm::Result<()> {
		if output.is_tty() {
			trace!("setting terminal title to {:?}", title);
			crossterm::execute!(output, crossterm::terminal::SetTitle(title))?;
		}
		Ok(())
	}
}

/// Maps keyboard key events to a character sequence to send to a terminal.
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

// returns the parsed prompt, and if the prompt is the last segment in the data (except for whitespace)
fn parse_terminal_prompt<'a>(data: &'a [u8]) -> Option<(&'a str, bool)> {
	if let Ok(s) = std::str::from_utf8(data) {
		let prompt = s.char_indices()
			// find last if possible
			.rev() 
			// start going from starts of lines
			.filter(|&(_, c)| c == '\r' || c == '\n') 
			// trim starting whitespace, trim at 64+2 chars:
			// hostnames cannot be bigger than 64, plus 2 for prompt character
			.map(|(i, _)| {
				let s = s[i..].trim_start();
				// trim to 64 chars + 2 (only need 1?) for prompt character
				let end = s.len().min(64+2);
				&s[..end]
			}) 
			.filter_map(|s| {
				// IOS+Linux prompts
				let end = s.find(|c| c == '#' || c == '>' || c == '$');
				end
					.filter(|i| {
						// according to ASA: must start/end with alphanumeric, middle can contain dashes
						// IOS allows middle underscores
						let prompt = &s[..*i];
						prompt.starts_with(|c: char| c.is_ascii_alphanumeric()) &&
						prompt.ends_with(|c: char| c.is_ascii_alphanumeric()) &&
						s[1..i-1].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
					})
					.map(|i| &s[..i+1])
			})
			// get first (last because .rev()) found prompt
			.next();

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
			trace!("{} : Binary ({} B) : {:?} : {:?}", id_str, as_hex.len(), truncate_string(&as_hex, 10), truncate_string(&s, 10));
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
