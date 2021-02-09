#![feature(async_closure)]
#![feature(str_split_once)]
#![feature(try_blocks)]

use std::{cell::Cell, time::Instant};
use std::borrow::Cow;
use log::{error, warn, info, debug, trace};
use clap::Clap;
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


use cml::rest::CmlUser;

type CmlResult<T> = Result<T, cml::rest::Error>;

use cmlterm::listing::*;

#[derive(Clap)]
#[clap(version = "0.1.0", author = "Chris M. <35407569+csm123199@users.noreply.github.com>")]
struct Args {
	//#[clap(long)]
	//vnc: bool,

	#[clap(subcommand)]
	subc: SubCmd,
}

#[derive(Clap)]
enum SubCmd {
	List,
	Open(SubCmdOpen),
	Run(SubCmdRun),
	Expose(SubCmdExpose),
}

#[derive(Clap)]
struct SubCmdOpen {
	uuid_or_lab: String,
	device: Option<String>,
	line: Option<u8>,
}

#[derive(Clap)]
struct SubCmdRun {
	uuid: String,
	cmds: Vec<String>,
}

#[derive(Clap)]
struct SubCmdExpose {
	uuid: String,

	#[clap(short, long)]
	port: Option<u16>,

	#[clap(short, long)]
	address: Option<std::net::IpAddr>,
}
impl SubCmdExpose {
	async fn run(&self, host: &str) {
		// open port
		// open websocket per port - let the CML multiplexer do the multiplexing

		let listen_addr = self.address.unwrap_or([127, 0, 0, 1].into());
		let listen_port = self.port.unwrap_or(0);

		let listener = tokio::net::TcpListener::bind((listen_addr, listen_port)).await.unwrap();
		eprintln!("Listening on {}...", listener.local_addr().unwrap());

		tokio_stream::wrappers::TcpListenerStream::new(listener)
			.for_each_concurrent(None, |sock| {
				//let (stream, peer) = sock.expect("error obtaining new tcp connection");
				let stream = sock.expect("error obtaining new tcp connection");
				SubCmdExpose::forward_tcp_to_console(stream, host, &self.uuid)
			}).await;

		info!("done accepting/processing connections");
	}
	
	/// Pipes an existing TCP connection to a console device at the specified CML host/device UUID pair
	///
	/// Note: if using a terminal, set the following settings for the best experience
	/// * Local echo: false
	/// * Local line editing: false
	/// * Nagles algorithm: disabled
	async fn forward_tcp_to_console(mut stream: TcpStream, host: &str, uuid: &str) {
		use tokio::io::{AsyncWriteExt, AsyncReadExt};

		let (ws_stream, _) = connect_to_console(host, uuid).await.unwrap();
		let peer_addr = stream.peer_addr().unwrap();
		info!("WS Server <-> {} : Established", peer_addr);

		// Note that this implementation must be a bit asymmetric
		// That is because we are exchanging between a frame-based
		// protocol (WebSockets) and a stream-based protocol (TCP).


		// send things immediatly
		stream.set_nodelay(true).unwrap();

		// receive binary messages
		// send text messages
		// respond to server pings as necessary
		// close TCP on WS close request
		// close WS on TCP closed

		// split websocket/tcp into seperate write/read streams
		let (ws_write, mut ws_read) = ws_stream.split();
		let (mut tcp_read, mut tcp_write) = stream.split();

		// create a channel to pipe messages to WS through
		//let (server_tx, server_rx) = futures_channel::mpsc::unbounded();

		// create a channel to pipe messages to WS through
		let (to_serv, serv_rx) = futures_channel::mpsc::unbounded(); // replace with ::channel() - bounded version?
		let mut to_ws_from_tcp = to_serv.clone();
		let mut to_ws_from_ws = to_serv;

		// Use a cell so the different async blocks can both immutably borrow requested_close
		let requested_close = Cell::new(false);

		let serv_to_client = async {
			// will return None with the stream is exhausted
			while let Some(msg) = ws_read.next().await {
				let msg = msg.unwrap();
				match msg {
					Message::Text(t) => warn!("unexpected text message from server: {:?}", t),
					Message::Pong(t) => warn!("unexpected pong message from server: {:?}", String::from_utf8_lossy(&t)),
					Message::Close(_) => {
						trace!("WS Server --> {} : {:?}", peer_addr, msg);
						if ! requested_close.get() {
							// the server has initiated the close - respond back
							to_ws_from_ws.unbounded_send(Message::Close(None)).unwrap();
						} else {
							// we initiated the initial close
						}
						break;
					},
					Message::Ping(t) => {
						to_ws_from_ws.unbounded_send(Message::Pong(t)).unwrap()
					},
					Message::Binary(b) => {
						let s = String::from_utf8_lossy(&b);
						trace!("WS Server --> {} : {:?}", peer_addr, s);
						tcp_write.write_all(&b).await.unwrap();
					},
				}
			}

			trace!("WS Server --> {} : Closing...", peer_addr);

			to_ws_from_ws.disconnect();
			tcp_write.flush().await.unwrap();
			tcp_write.shutdown().await.unwrap();
			
			debug!("WS Server --> {} : Closed", peer_addr);
		};

		let tcp_to_ws = async {
			let mut v = vec![0u8; 1024];
			
			loop {
				let n = tcp_read.read(&mut v).await.unwrap();
				if n == 0 {
					// we are done - request close
					requested_close.set(true);
					to_ws_from_tcp.unbounded_send(Message::Close(None)).unwrap();
					break;
				}
				let msg = String::from_utf8_lossy(&v[..n]);
				trace!("WS Server <-- {} : {:?}", peer_addr, msg);
				to_ws_from_tcp.unbounded_send(Message::text(msg)).unwrap();
			}

			
			debug!("WS Server <-- {} : Closed", peer_addr);
			to_ws_from_tcp.disconnect();
		};

		use futures::FutureExt;

		let ws_responder = serv_rx
			.inspect(|msg| if ! matches!(msg, Message::Pong(_)) { trace!("WS Server <-- {} : {:?}", peer_addr, msg) })
			.map(Ok)
			.forward(ws_write)
			.then(async move |_| trace!("WS Server <-- {} : WS write connection closed", peer_addr));

		futures_util::pin_mut!(serv_to_client, tcp_to_ws, ws_responder);
		let ((), (), _ws_send_res) = future::join3(serv_to_client, tcp_to_ws, ws_responder).await;
		// todo: error handling for ws_send_res ?

		info!("WS Server <-> {} : Closed", peer_addr);
		// send TCP -> WS
	}
}

async fn connect_to_console(host: &str, uuid: &str) -> Result<(WebSocketStream<TlsStream<TcpStream>>, WsResponse), WsError> {
	// connect to host
	let s: TcpStream = TcpStream::connect((host, 443)).await.unwrap();
	
	// unwrap ssl
	let connector = TlsConnector::builder()
		.danger_accept_invalid_certs(true)
		.build().unwrap();
	let connector = TlsConnectorAsync::from(connector);
	let s = connector.connect(host, s).await.unwrap();
	
	// connect using websockets
	let endpoint = format!("wss://{}/ws/dispatch/frontend/console?uuid={}", host, uuid);
	tokio_tungstenite::client_async(endpoint, s).await
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
fn log_ws_message(t: Instant, msg: &Message, id_str: &str, pings: bool) {
	match msg {
		Message::Text(s) => {
			let as_hex = hex::encode(s.as_bytes());
			debug!("[{:?}] {} : Text : {:?} : {:?}", Instant::now() - t, id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
		},
		Message::Binary(b) => {
			let as_hex = hex::encode(&b);
			let s = String::from_utf8_lossy(&b);
			debug!("[{:?}] {} : Binary : {:?} : {:?}", Instant::now() - t, id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
		},
		Message::Close(cf) => {
			debug!("[{:?}] {} : Close{}", Instant::now() - t, id_str, if let Some(c) = cf { format!(" : {:?}", c) } else { "".into() });
		},
		Message::Pong(b) => {
			if pings {
				let as_hex = hex::encode(&b);
				let s = String::from_utf8_lossy(&b);
				debug!("[{:?}] {} : Pong : {:?} : {:?}", Instant::now() - t, id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
			}
		},
		Message::Ping(b) => {
			if pings {
				let as_hex = hex::encode(&b);
				let s = String::from_utf8_lossy(&b);
				debug!("[{:?}] {} : Ping : {:?} : {:?}", Instant::now() - t, id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
			}
		}
	}
}

async fn open_terminal(t: Instant, host: &str, uuid: &str) {
	type WsSender = futures_channel::mpsc::UnboundedSender<Message>;
	//type WsReceiver = futures_channel::mpsc::UnboundedReceiver<Message>;

	async fn handle_terminal_input(t: Instant, tx: WsSender) {
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
				//debug!("[{:?}] (input event) {:?}", Instant::now() - t, &event_res);
				
				match event_res {
					Ok(event) => match event {
						Event::Key(kevent) => match cmlterm::event_to_code(kevent) {
							//Ok(c) => send(&tx, c.to_string()),
							Ok(c) => tx.unbounded_send(Message::Text(c.to_string())).unwrap(),
							Err(e) => warn!("[{:?}] unable to convert key code to sendable sequence: {}", Instant::now() - t, e),
						},
						c @ _ => warn!("[{:?}] unhandled terminal event: {:?}", Instant::now() - t, c),
					},
					Err(e) => error!("[{:?}] error occured from stdin: {:?}", Instant::now() - t, e)
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
			if let Some(reason) = r {
				eprintln!("Server has closed the stream: ({}, {:?})", reason.code, reason.reason);
			} else {
				eprintln!("Server has closed the stream.");
			}
			

			// TODO: close the stream somehow?
		} else {
			eprintln!("Unexpected websocket message type: {:?}", msg);
		}
	}

	let (ws_stream, _) = connect_to_console(host, uuid).await.unwrap();

	debug!("[{:?}] websocket established", Instant::now() - t);

	// create a channel to pipe stdin through
	let (server_tx, server_rx) = futures_channel::mpsc::unbounded();

	// stdin is block-on-read only, so spawn a new thread for it
	tokio::spawn(handle_terminal_input(t, server_tx.clone()));

	// split websocket into seperate write/read streams
	let (write, read) = ws_stream.split();
	let stdin_to_ws = server_rx
		.inspect(|msg| log_ws_message(t, msg, "send", false))
		.map(Ok)
		.forward(write);
	let ws_to_stdout = read
		.inspect(|msg| if let Ok(m) = msg { log_ws_message(t, m, "recv", false) })
		.for_each(|msg| handle_ws_msg(&server_tx, msg));

	// will overwrite the console title based on received messages
	set_terminal_title("CML Console").unwrap();

	// reprint the current line for the prompt/currently typed line + signal to user that we are ready
	server_tx.unbounded_send(Message::Text(ascii::AsciiChar::FF.to_string())).unwrap(); 
	
	futures_util::pin_mut!(stdin_to_ws, ws_to_stdout);
	future::select(stdin_to_ws, ws_to_stdout).await;
}

#[tokio::main]
async fn main() -> CmlResult<()> {
	use std::time::Instant;
	let t = Instant::now();
	env_logger::init();

	trace!("[{:?}] parsing args", Instant::now() - t);
	// can eventually 'execve ssh' ?
	let args = Args::parse();

	let auth = cml::get_auth_env().expect("Unable to get authentication info from environment");
	
	match &args.subc {
		SubCmd::List => {
			trace!("[{:?}] logging in", Instant::now() - t);
			let client = auth.login().await?;
			let client: &CmlUser = &client;
			trace!("[{:?}] listing data", Instant::now() - t);

			let lab_entries = get_node_listing_data(&client, t).await?;
			list_data(lab_entries, t);
		},
		SubCmd::Open(open) => {
			open_terminal(t, &auth.host, &open.uuid_or_lab).await;
		},
		SubCmd::Run(_run) => {
			todo!("running individial/batch commands non-interactively");

/*
			let (mut ws_stream, _) = connect_to_console(&auth.host, &run.uuid).await.unwrap();
			eprintln!("websocket established");

			let (write, read) = ws_stream.split();

			let (server_tx, server_rx) = futures_channel::mpsc::unbounded();
			let to_serv = server_rx.forward(write);

			let refresh_terminal: String = [
				AsciiChar::FF.as_char().to_string(), // reprint line
				AsciiChar::SOH.as_char().to_string(), // go home
				"! ".into(), // insert "! "
				"\n".into(), // enter
				AsciiChar::ETX.as_char().to_string(), // ctrl+c to go to #
			].iter().map(|s| s.as_str()).collect();

			server_tx.unbounded_send(Ok(Message::Text(refresh_terminal))).unwrap();
			
			read
				.for_each(|msg| {

				})

			//read.

			//let (write, read) = ws_stream.split();
			//write.send("asd").unwrap();

			// read & throw away for 500ms, then continue
			let timeout = tokio::time::delay_for(std::time::Duration::from_millis(500));

			futures_util::pin_mut!(to_serv, timeout, read);
			loop {
				futures::select! {
					() = to_serv => {},
					() = timeout => {},
				}
			}
			

			todo!();
			*/
		},
		SubCmd::Expose(expose) => { expose.run(&auth.host).await; }
	}

	Ok(())
}

/*
	// lists available devices
	cmlterm [--vnc] list

	// opens an interactive terminal session with the specified device
 	cmlterm [--vnc] open <UUID>
	cmlterm [--vnc] open <LAB_NAME> <DEVICE_NAME> [LINE_NUMBER = 0]
	cmlterm [--vnc] open <LAB_ID> <DEVICE_ID> [LINE_NUMBER = 0]

	// opens a port for other applications to connect to interact the the device like telnet
	cmlterm [--vnc] pipe <UUID>
	cmlterm [--vnc] pipe <LAB_NAME> <DEVICE_NAME> [LINE_NUMBER = 0] // prints {"host": "localhost", "port": XXXXX}
	cmlterm [--vnc] pipe <LAB_ID> <DEVICE_ID> [LINE_NUMBER = 0] // prints {"host": "localhost", "port": XXXXX}

	// runs a sequence of commands from arguments, or stdin, to run on the device
	cmlterm run <UUID>
	cmlterm run <LAB_NAME> <DEVICE_NAME> [COMMAND, ...] (or over stdin)
	cmlterm run <LAB_ID> <DEVICE_ID> [COMMAND, ...] (or over stdin)
*/



