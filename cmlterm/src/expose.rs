use clap::Clap;
use futures_util::{future, StreamExt};
use log::{debug, error, info, trace, warn};
use std::cell::Cell;
use tokio::net::TcpStream;

use tokio_tungstenite::tungstenite;
use tungstenite::protocol::Message;

#[derive(Clap)]
pub struct SubCmdExpose {
	#[clap(short, long)]
	vnc: bool,

	uuid: String,

	#[clap(short, long)]
	port: Option<u16>,

	#[clap(short, long)]
	address: Option<std::net::IpAddr>,
}
impl SubCmdExpose {
	pub async fn run(&self, host: &str) {
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
		use tokio::io::{AsyncReadExt, AsyncWriteExt};

		let (ws_stream, _) = crate::connect_to_console(host, uuid).await.unwrap();
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
                    Message::Pong(t) => warn!(
                        "unexpected pong message from server: {:?}",
                        String::from_utf8_lossy(&t)
                    ),
					Message::Close(_) => {
						trace!("WS Server --> {} : {:?}", peer_addr, msg);
                        if !requested_close.get() {
							// the server has initiated the close - respond back
							to_ws_from_ws.unbounded_send(Message::Close(None)).unwrap();
						} else {
							// we initiated the initial close
						}
						break;
                    }
                    Message::Ping(t) => to_ws_from_ws.unbounded_send(Message::Pong(t)).unwrap(),
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
            .inspect(|msg| {
                if !matches!(msg, Message::Pong(_)) {
                    trace!("WS Server <-- {} : {:?}", peer_addr, msg)
                }
            })
			.map(Ok)
			.forward(ws_write)
            .inspect(|_| trace!("WS Server <-- {} : WS write connection closed", peer_addr));

		futures_util::pin_mut!(serv_to_client, tcp_to_ws, ws_responder);
		let ((), (), _ws_send_res) = future::join3(serv_to_client, tcp_to_ws, ws_responder).await;
		// todo: error handling for ws_send_res ?

		info!("WS Server <-> {} : Closed", peer_addr);
		// send TCP -> WS
	}
}
