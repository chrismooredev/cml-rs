
use std::borrow::Cow;

use anyhow::Context;
use thiserror::Error;
use futures::{SinkExt, StreamExt};
use log::{debug, error, warn};
use crossterm::terminal;

use crate::term::{BoxedDriverError, UserTUI};
use crate::term::common::{ConsoleDriver, ConsoleUpdate};

/// A raw terminal
///
/// Essentially a pipe between the console driver and stdio
///
/// Note that since we do not process terminal input ourselves (only forward),
/// a program will have to close stdin deliberately, notably, a user cannot send EOF using keystrokes
pub struct RawTerminal<UTUI: UserTUI> {
	driver: ConsoleDriver,
	tui: UTUI,
}

#[derive(Debug, Error)]
pub enum RawError {
	#[error("An unrecoverable IO has error occured.")]
	Io(#[from] std::io::Error),
	#[error("A console connection error occured.")]
	Connection(#[from] BoxedDriverError),
}

impl<UTUI: UserTUI> RawTerminal<UTUI> {
	pub fn new(driver: ConsoleDriver, tui: UTUI) -> RawTerminal<UTUI> {
		RawTerminal {
			driver,
			tui,
		}
	}

	/// Runs drives the console based off the current process' stdin and stdout.
	/// Recieves events from stdin in a loop, and passes it to the console.
	/// Data from the console is sent back to stdout, setting the terminal's title as appropriate.
	pub async fn run(self) -> Result<(), RawError> {
		use futures::{AsyncReadExt, AsyncWriteExt};

		let RawTerminal { driver, tui } = self;
		// push stdin into it
		// write output to stdout, with some buffering
		let mut tui = tui;
		let mut driver = driver.fuse();

		// tools will 'fake' using a tty reader, so check our output
		if tui.is_tty_writer() {
			eprintln!("Note: Since raw mode uses a raw terminal, the process must be stopped externally if using interactively");
		}

		terminal::enable_raw_mode().with_context(|| "enabling terminal's raw mode")?;
		let mut userbuf = [0u8; 2048];
		let mut userdone = false;

		loop {
			tokio::select! {
				userchunk_res = tui.read(&mut userbuf), if !userdone => {
					match userchunk_res? {
						0 => {
							debug!("tui input sent EOF");
							userdone = true;
							driver.close().await?;
						},
						n @ _ => {
							let data = &userbuf[..n];
							let as_str = String::from_utf8_lossy(data);
							if let Cow::Owned(_) = as_str {
								warn!("input data was not UTF8. Sending lossy UTF8 to server.");
							}
							driver.send(as_str.to_string()).await?;
						},
					}
				},
				Some(termchunk_res) = driver.next() => {
					let ConsoleUpdate { last_chunk, .. } = termchunk_res?;
					let data = last_chunk.as_slice();
					
					tui.write_all(data).await?;
					tui.flush().await?;
				},

				else => {
					debug!("tokio input loop finished, breaking");
					break
				},
			}
		}

		Ok(())
	}
}
