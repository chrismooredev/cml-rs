#![feature(async_closure)]
#![feature(str_split_once)]
#![feature(try_blocks)]

use std::time::Instant;
use std::borrow::Cow;
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
			eprintln!("[{:?}] {} : Text : {:?} : {:?}", Instant::now() - t, id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
		},
		Message::Binary(b) => {
			let as_hex = hex::encode(&b);
			let s = String::from_utf8_lossy(&b);
			eprintln!("[{:?}] {} : Binary : {:?} : {:?}", Instant::now() - t, id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
		},
		Message::Close(cf) => {
			eprintln!("[{:?}] {} : Close{}", Instant::now() - t, id_str, if let Some(c) = cf { format!(" : {:?}", c) } else { "".into() });
		},
		Message::Pong(b) => {
			if pings {
				let as_hex = hex::encode(&b);
				let s = String::from_utf8_lossy(&b);
				eprintln!("[{:?}] {} : Pong : {:?} : {:?}", Instant::now() - t, id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
			}
		},
		Message::Ping(b) => {
			if pings {
				let as_hex = hex::encode(&b);
				let s = String::from_utf8_lossy(&b);
				eprintln!("[{:?}] {} : Ping : {:?} : {:?}", Instant::now() - t, id_str, truncate_string(&as_hex, 10), truncate_string(&s, 10));
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
				//eprintln!("[{:?}] (input event) {:?}", Instant::now() - t, &event_res);
				//eprintln!("{}", _s);
				
				match event_res {
					Ok(event) => match event {
						Event::Key(kevent) => match cmlterm::event_to_code(kevent) {
							//Ok(c) => send(&tx, c.to_string()),
							Ok(c) => tx.unbounded_send(Message::Text(c.to_string())).unwrap(),
							Err(e) => eprintln!("[{:?}] unable to convert key code to sendable sequence: {}", Instant::now() - t, e),
						},
						c @ _ => eprintln!("[{:?}] unhandled terminal event: {:?}", Instant::now() - t, c),
					},
					Err(e) => eprintln!("[{:?}] error occured from stdin: {:?}", Instant::now() - t, e)
				}
			})
			.await;
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
			let str_end = String::from_utf8_lossy(if data.len() > 32 { &data[data.len()-32..] } else { &data });
			//eprintln!("message end: {:?}", str_end);
			let str_end = str_end.trim_end(); // some devices have spaces afterward
			if str_end.ends_with(|c| c == '#' || c == '>' || c == '$') { // IOS+Linux prompts
				if let Some(line_start) = str_end.rfind(|c| c == '\r' || c == '\n') {
					// get prompt from start of line to end (not including space)
					let prompt = &str_end[line_start..].trim();
					//eprintln!("parsed prompt: {:?}", prompt);
					crossterm::execute!(
						lock,
						crossterm::terminal::SetTitle(prompt)
					).unwrap();
				}
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

	eprintln!("[{:?}] websocket established", Instant::now() - t);

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

	eprintln!("[{:?}] parsing args", Instant::now() - t);
	// can eventually 'execve ssh' ?
	let args = Args::parse();

	let auth = cml::get_auth_env().expect("Unable to get authentication info from environment");
	
	match &args.subc {
		SubCmd::List => {
			eprintln!("[{:?}] logging in", Instant::now() - t);
			let client = auth.login().await?;
			let client: &CmlUser = &client;
			eprintln!("[{:?}] listing data", Instant::now() - t);

			let lab_entries = get_node_listing_data(&client, t).await?;
			list_data(lab_entries, t);
		},
		SubCmd::Open(open) => {
			open_terminal(t, &auth.host, &open.uuid_or_lab).await;
		},
		SubCmd::Run(run) => {
			let (ws_stream, _) = connect_to_console(&auth.host, &run.uuid).await.unwrap();
			eprintln!("websocket established");

			todo!();
		}
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



