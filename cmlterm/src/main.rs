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

type CmlResult<T> = Result<T, cml::rest::Error>;

use cmlterm::listing::SubCmdList;
use cmlterm::open_term::SubCmdOpen;
use cmlterm::expose::SubCmdExpose;


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
	List(SubCmdList),
	Open(SubCmdOpen),
	Run(SubCmdRun),
	Expose(SubCmdExpose),
}

#[derive(Clap)]
struct SubCmdRun {
	uuid: String,
	cmds: Vec<String>,
}


#[tokio::main]
async fn main() -> CmlResult<()> {
	env_logger::init();

	trace!("parsing args");
	// can eventually 'execve ssh' ?
	let args = Args::parse();

	let auth = cml::get_auth_env().expect("Unable to get authentication info from environment");
	
	match &args.subc {
		SubCmd::List(list) => list.run(&auth).await.unwrap(),
		SubCmd::Open(open) => open.run(&auth.host).await,
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
		SubCmd::Expose(expose) => expose.run(&auth.host).await,
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



