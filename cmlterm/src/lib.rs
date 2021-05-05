
use std::collections::VecDeque;
use thiserror::Error;
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
pub mod api;
pub mod terminal;
pub mod term;
pub mod open_term;

#[derive(Debug, Error)]
pub enum TerminalError {
	#[error("An error occured invoking the CML Rest API")]
	Cml(#[from] cml::rest::Error),
	#[error("Bad command line usage")]
	BadUsage,
	#[error("Unable to find the node within the lab")]
	BadDeviceQuery(#[from] terminal::NodeSearchError),
	#[error("Line not found on resolved node")]
	BadLineQuery(#[from] terminal::ConsoleSearchError),
	#[error("Completions script not available for {}", .0)]
	UnsupportedCompletionShell(String),
}

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


// misc private utilities

/// Adds data to the rotating buffer, and makes it contiguous.
/// Only fills the buffer to the buffer's capacity, and rotates data in terms of that.
/// In other words, this function will not allocate.
fn update_chunkbuf(buffer: &mut VecDeque<u8>, chunk: &[u8]) {
	// update our cache, later notify listeners

	// shorten the VecDeque to a length capable of holding the data without exceeding capacity
	// then fill the VecDeque with our new data chunk

	if buffer.len() + chunk.len() <= buffer.capacity() {
		// we can hold this chunk without removing elements
		buffer.extend(chunk);
	} else if chunk.len() > buffer.capacity() {
		// this will extend capacity - we want to be able to hold at least the size of each chunk
		buffer.clear();
		buffer.extend(chunk);
	} else {
		// we must delete some elements to hold this chunk
		let start_ind = buffer.capacity() - chunk.len();
		let rotate_amt = buffer.len() .min( chunk.len() );
		
		buffer.rotate_left(rotate_amt);
		buffer.resize(start_ind, 0);
		buffer.extend(chunk);
	}

	buffer.make_contiguous();
}

fn find_slice<T>(haystack: &[T], needle: &[T]) -> Option<usize>
	where for<'a> &'a [T]: PartialEq
{
	// https://stackoverflow.com/a/35907071/11536614
	haystack.windows(needle.len()).position(|window| window == needle)
}

