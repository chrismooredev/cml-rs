#![feature(or_patterns)]
#![feature(async_closure)]
#![feature(option_expect_none)]

use std::{cell::Cell, time::Instant};
use std::borrow::Cow;
use log::{error, warn, info, debug, trace};
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

use ascii::{AsciiChar};
// use termion::event::{Event, Key};
use crossterm::event::{ KeyCode, KeyEvent, KeyModifiers };
use smol_str::SmolStr;
use clap::Clap;


macro_rules! esc {
	($s: literal) => {
		// AsciiChar::ESC + $s
		SmolStr::new_inline(concat!('\x1B', $s))
	}
}

pub mod listing;
pub mod open_term;
pub mod expose;


/// Connects to the given console UUID using secured websockets (accepting invalid/self-signed certificates).
pub async fn connect_to_console(host: &str, uuid: &str) -> Result<(WebSocketStream<TlsStream<TcpStream>>, WsResponse), WsError> {
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


