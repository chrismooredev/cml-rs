
use std::time::Duration;

use clap::Clap;
use crossterm::tty::IsTty;
use merge_io::MergeIO;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use cml::rest_types::State;
use cml::rest::Authenticate;
use crate::terminal::{ConsoleCtx, ConsoleSearchError, NodeCtx};
use crate::TerminalError;


#[derive(Clap)]
pub struct SubCmdOpen {
	//#[clap(short, long)]
	//_vnc: bool,

	/// Boots the machine if necessary. If autocompleting, also shows all available devices.
	#[clap(short, long)]
	boot: bool,

	/// Does not affect functionality, shows lab/node IDs when auto-completing
	#[clap(short, long)]
	_ids: bool,

	/// Does not affect functionality, shows console UUIDs when auto-completing
	#[clap(short, long)]
	_uuids: bool,

	/// Expose a device's raw console to stdio, without buffering. Suitable for fine-grained device scripting.
	#[clap(short, long)]
	raw: bool,

	/// Uses CML's SSH terminal instead of the websocket terminal the UI uses. (Experimental)
	#[clap(short, long)]
	ssh: bool,

	/// Accepts a combination of ID or label for labs and nodes, or the console's UUID
	#[clap(value_name = "/lab/node[/line = 0]")]
	uuid_or_lab: String,
}
impl SubCmdOpen {
	pub async fn run(&self, auth: &Authenticate) -> Result<(), TerminalError> {
		// TODO: if necessary, request UUID from lab/device/line
		let dest = &self.uuid_or_lab;

		let client = auth.login().await?;

		let context: Option<ConsoleCtx> = if ! dest.starts_with('/') {
			// find by UUID
			let keys = client.keys_console(true).await?;
			let console = keys.into_iter()
				.find(|(k, _)| k == dest);
			
			match console {
				Some((uuid, cons)) => {
					// refetch the UUID for this node using a validated ConsoleCtx
					let node = NodeCtx::search(&client, &cons.lab_id, &cons.node_id).await??;
					let ctx = ConsoleCtx::find_line(&client, &node, Some(cons.line)).await?.ok();
					if let Some(ctx) = &ctx {
						assert_eq!(ctx.uuid(), uuid, "validated UUID is not the same as provided UUID");
					}
					ctx
				},
				None => Err(ConsoleSearchError::NoMatchingLines)?,
			}
		} else {
			// find by split names
			let split: Vec<_> = dest.split('/').collect();
			let indiv = match split.as_slice() {
				&["", lab, node] => Some((lab, node, None)),
				&["", lab, node, line] => Some((lab, node, Some(line))),
				_ => None,
			};

			if let Some((lab_desc, node_desc, line)) = indiv {
				let node = NodeCtx::search(&client, lab_desc, node_desc).await??;
				let line: Option<u64> = line.map(|s| s.parse().expect("Unable to parse line as number"));

				match ConsoleCtx::find_line(&client, &node, line).await? {
					Ok(ctx) => Some(ctx),
					Err(ConsoleSearchError::NodeNotActive) if self.boot => {
						let is_tty = std::io::stdout().is_tty();

						eprint!("Node /{}/{} is currently {}. Will connect once booted...", node.lab().1, node.node().1, node.meta().state);

						let _res = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(&format!("CML Booting {}", node.node().1)));

						client.lab_node_start(node.lab().0, node.node().0).await?;

						let ctx = loop {
							tokio::time::sleep(Duration::from_secs(1)).await;

							let ctx = ConsoleCtx::find_line(&client, &node, line).await?;
							match ctx {
								Ok(ctx) => {
									eprintln!(""); // finish dots
									break Some(ctx);
								},
								Err(ConsoleSearchError::NodeNotActive) => eprint!("."),
								Err(e @ _) => Err(e)?,
							}
						};

						if ! is_tty {
							eprint!("Node has been started. Script waiting for it to be fully booted...");
							loop {
								tokio::time::sleep(Duration::from_secs(1)).await;
	
								let state = client.lab_node_state(node.lab().0, node.node().0).await?.expect("node was deleted");
								if state == State::Booted {
									eprintln!("");
									break;
								} else {
									eprint!(".");
								}
							}
						}

						ctx
					},
					Err(e) => Err(e)?,
				}
			} else {
				// show usage?
				todo!("please specify /<lab id or name>/<node id or name>/[line num = 0]");
			}
		};

		if let Some(ctx) = context {
			use futures::stream::StreamExt;
			use crate::term::common::ConsoleDriver;
			use crate::term::frontend::types::{UserTerminal, ScriptedTerminal, RawTerminal};
			use crate::term::Drivable;

			// TODO: check for println in these drivers, and remove them
			let console_stream: Box<dyn Drivable> = if ! self.ssh {
				use crate::term::backend::websocket::WsConsole;
				Box::new(WsConsole::new(&ctx).await.unwrap().fuse())
			} else {
				use crate::term::backend::ssh::SshConsole;
				Box::new(SshConsole::single(&ctx, &auth).await.unwrap().fuse())
			};

			let user = MergeIO::new(
				tokio::io::stdin().compat(),
				tokio::io::stdout().compat_write()
			);
			
			let driver = ConsoleDriver::from_connection(ctx, console_stream);

			if self.raw {
				let rt = RawTerminal::new(driver, user);
				rt.run().await.expect("an error within the raw stdin-driven terminal program");
			} else if std::io::stdin().is_tty() {
				// Note: uses the terminals 'raw' mode - it reads events, not data stream
				// as such, it will read events directly from tty/stdin, instead of the provided reader
				let ut = UserTerminal::new(driver, user).map_err(|e| anyhow::Error::new(e))?;
				ut.run().await.expect("an error within the tty stdin-driven terminal program");
			} else {
				let st = ScriptedTerminal::new(driver);
				st.run().await.expect("an error within scripted stdin-driven terminal program");
			}
		} else {
			todo!("no console device found for descriptor");
		}

		Ok(())
	}
}
