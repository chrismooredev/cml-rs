
use std::{fmt::Debug, pin::Pin, task::{Context, Poll}};

use futures::{Sink, SinkExt, Stream};
use futures::StreamExt;
use log::{debug, error, trace, warn};

use tokio::net::TcpStream;
use tokio_native_tls::TlsStream;
use tokio_tungstenite::{WebSocketStream, tungstenite};
use native_tls::TlsConnector;
use tokio_native_tls::native_tls;
use tokio_native_tls::TlsConnector as TlsConnectorAsync;
use tungstenite::error::{Error as WsError, Result as WsResult};
use tungstenite::protocol::Message;
use tungstenite::handshake::client::Response as WsResponse;

use crate::terminal::ConsoleCtx;

/// A websocket wrapper to handle our use case, communicating with CML console lines
///
/// Specifically asserts we get messages from Binary WS frames, and send as Text WS frames
#[derive(Debug)]
pub struct WsConsole {
	ws: WebSocketStream<TlsStream<TcpStream>>,
	ws_closed: bool,
}
impl WsConsole {
	pub async fn new(console: &ConsoleCtx) -> Result<WsConsole, WsError> {
		// safety: we only get ConsoleCtx structs with valid UUIDs by searching for devices
		let (ws_stream, resp) = unsafe { WsConsole::connect_raw(console.node().host(), &console.uuid()).await? };

		debug!("websocket established (HTTP status code {:?})", resp.status());
		trace!("websocket headers:");
		for header in resp.headers().iter() {
			trace!("\t{:?}", header);
		}

		Ok(WsConsole {
			ws: ws_stream,
			ws_closed: false,
		})
	}

	/// Attempts to connect directly to the specified UUID
	///
	/// # Safety
	/// If the UUID is malformed, the CML-side console multiplexor may crash and the instance will have to be rebooted.
	async unsafe fn connect_raw(host: &str, uuid: &str) -> Result<(WebSocketStream<TlsStream<TcpStream>>, WsResponse), WsError> {
		// connect to host
		let s: TcpStream = TcpStream::connect((host, 443)).await.unwrap();

		// unwrap ssl
		let connector = TlsConnector::builder()
			.danger_accept_invalid_certs(true)
			.build().unwrap();
		let connector = TlsConnectorAsync::from(connector);
		let s = connector.connect(host, s).await.unwrap();

		// TODO: validate UUID first, otherwise we may crash the VIRL2 Device mux service
		//       possible to do without authenticating and quering for console keys?

		// connect using websockets
		// uses same endpoint as HTML UI/breakout tool
		let endpoint = format!("wss://{}/ws/dispatch/frontend/console?uuid={}", host, uuid);
		tokio_tungstenite::client_async(endpoint, s).await
	}
}

impl Stream for WsConsole {
	type Item = WsResult<Vec<u8>>;
	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		// only have to worry about text messages
		// tokio-tungstenite will handle websocket ping/pongs and close messages

		loop {
			match self.ws.poll_next_unpin(cx) {
				Poll::Pending => return Poll::Pending,
				Poll::Ready(None) => {
					debug!("WebSocket remote has closed");
					self.ws_closed = true;
					return Poll::Ready(None);
				},
				Poll::Ready(Some(Err(e))) => {
					return Poll::Ready(Some(Err(e)));
				},
				Poll::Ready(Some(Ok(Message::Binary(odata)))) => {
					return Poll::Ready(Some(Ok(odata)));
				},
				Poll::Ready(Some(Ok(Message::Text(_)))) => {
					warn!("Recieved unexpected text frame from console websocket, this will be ignored (we should get our data in Binary frames");
					// loop around again
				},
				Poll::Ready(Some(Ok(Message::Close(_) | Message::Ping(_) | Message::Pong(_)))) => {
					// loop around again
				},
			}
		}
	}
}
impl Sink<String> for WsConsole {
	type Error = WsError;
	fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		// prepare to send a value
		// producers must receive Ready(Ok(())) before calling .send
		self.ws.poll_ready_unpin(cx)
	}
	fn start_send(mut self: Pin<&mut Self>, item: String) -> Result<(), Self::Error> {
		// send data to server
		self.ws.start_send_unpin(Message::Text(item))
	}
	fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		// flush data to server
		self.ws.poll_flush_unpin(cx)
	}
	fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		// Close the underlying sink. Will also send the websocket close frame.
		let result = self.ws.poll_close_unpin(cx);
		
		if let Poll::Ready(Ok(())) = result {
			self.ws_closed = true;
		};

		result
	}
}
impl std::ops::Drop for WsConsole {
	fn drop(&mut self) {
		if ! self.ws_closed {
			error!("Dropped WsConsole without closing the sink. This should be considered a programming error.");
		}
	}
}
