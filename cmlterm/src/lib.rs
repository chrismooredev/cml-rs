
use native_tls::TlsConnector;
use tokio::net::TcpStream;
use tokio_native_tls::native_tls;
use tokio_native_tls::TlsConnector as TlsConnectorAsync;

use tokio_native_tls::TlsStream;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::WebSocketStream;
use tungstenite::error::Error as WsError;
use tungstenite::handshake::client::Response as WsResponse;

macro_rules! esc {
	($s: literal) => {
		// AsciiChar::ESC + $s
		SmolStr::new_inline(concat!('\x1B', $s))
	};
}

pub mod expose;
pub mod listing;
pub mod open_term;

/// Connects to the given console UUID using secured websockets (accepting invalid/self-signed certificates).
///
/// ### Safety:
/// Assumes the passed UUID is valid for the host. Passing an invalid/inactive UUID may crash the CML console multiplexor service.
pub async fn connect_to_console(host: &str, uuid: &str) -> Result<(WebSocketStream<TlsStream<TcpStream>>, WsResponse), WsError> {
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
	let endpoint = format!("wss://{}/ws/dispatch/frontend/console?uuid={}", host, uuid);
	tokio_tungstenite::client_async(endpoint, s).await
}
