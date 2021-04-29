
use clap::Clap;
use crossterm::tty::IsTty;

use cml::rest::Authenticate;
use crate::terminal::{NodeCtx, ConsoleCtx};
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

	/// Accepts a combination of ID or label for labs and nodes
	#[clap(value_name = "/lab/node[/line = 0]")]
	uuid_or_lab: String,
}
impl SubCmdOpen {
	pub async fn run(&self, auth: &Authenticate) -> Result<(), TerminalError> {
		// TODO: if necessary, request UUID from lab/device/line
		let dest = &self.uuid_or_lab;

		let client = auth.login().await?;

		let context = if ! dest.starts_with('/') {
			// find by UUID
			let keys = client.keys_console(true).await?;
			let console = keys.into_iter()
				.find(|(k, _)| k == dest);
			
			match console {
				Some((uuid, cons)) => {
					let node = NodeCtx::search(&client, &cons.lab_id, &cons.node_id).await??;
					Some(ConsoleCtx::new(node, cons.line, uuid))
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
						eprint!("Node is currently {}. Will connect once booted...", node.meta().state);
						client.lab_node_start(node.lab().0, node.node().0).await?;

						loop {
							tokio::time::sleep(Duration::from_secs(1)).await;

							let ctx = ConsoleCtx::find_line(&client, &node, line).await?;
							match ctx {
								Ok(ctx) => {
									eprintln!(""); // finish dots
									break Some(ctx);
								},
								_ => {
									eprint!(".");
								},
							}
						}
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
			use crate::term::backend::websocket::WsConsole;
			use crate::term::common::ConsoleDriver;
			use crate::term::frontend::types::{UserTerminal, ScriptedTerminal, RawTerminal};

			let stream = WsConsole::new(&ctx).await.unwrap();
			let driver = ConsoleDriver::from_connection(ctx, Box::new(stream.fuse()));
			if self.raw {
				let rt = RawTerminal::new(driver);
				rt.run().await.expect("an error within the raw stdin-driven terminal program");
			} else if std::io::stdin().is_tty() {
				let ut = UserTerminal::new(driver);
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
