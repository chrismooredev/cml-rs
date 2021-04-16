
use clap::Clap;
use crossterm::tty::IsTty;

use cml::rest::Authenticate;
use crate::terminal::{NodeCtx, ConsoleCtx};
use crate::TerminalError;


#[derive(Clap)]
pub struct SubCmdOpen {
	#[clap(short, long)]
	_vnc: bool,

	/// Boots the machine if necessary. If autocompleting, also shows all available devices.
	#[clap(short, long)]
	boot: bool,

	/// If accepting input over stdin, then waits for prompt before sending next command
	#[clap(short, long)]
	wait: bool,

	/// Does not affect functionality, shows lab/node IDs when auto-completing
	#[clap(short, long)]
	_ids: bool,

	/// Does not affect functionality, shows console UUIDs when auto-completing
	#[clap(short, long)]
	_uuids: bool,

	/// Expose a device's raw console to stdio, without buffering
	#[clap(short, long)]
	raw: bool,

	uuid_or_lab: String,
}
impl SubCmdOpen {
	pub async fn run(&self, auth: &Authenticate) -> Result<(), TerminalError> {
		// TODO: if necessary, request UUID from lab/device/line
		let dest = &self.uuid_or_lab;

		let client = auth.login().await?;
		let keys = client.keys_console(true).await?;

		// find by UUID
		let mut dest_uuid = keys.iter()
			.find(|(k, _)| k == &dest)
			.map(|(k, _)| k.as_str());
		let mut context: Option<ConsoleCtx> = None;

		// find by split names
		if let None = dest_uuid {
			let split: Vec<_> = dest.split('/').collect();
			let indiv = match split.as_slice() {
				&["", lab, node] => Some((lab, node, None)),
				&["", lab, node, line] => Some((lab, node, Some(line))),
				_ => None,
			};

			if let Some((lab_desc, node_desc, line)) = indiv {
				let node = NodeCtx::search(&client, lab_desc, node_desc).await?;
				
				let node: NodeCtx = match node {
					Ok(nc) => nc,
					Err(nse) => return Err(nse.into()),
				};

				let line: Option<u64> = line.map(|s| s.parse().expect("Unable to parse line as number"));
				let console = node.resolve_line(&client, line, Some(&keys)).await?;

				let console: ConsoleCtx = match console {
					Ok(cc) => cc,
					Err(cse) => return Err(cse.into()),
				};

				dest_uuid = Some(keys.get_key_value(console.uuid()).unwrap().0.as_ref());
				context = Some(console);
			}
		}

		if let Some(ctx) = context {
			use futures::stream::StreamExt;
			use crate::term::backend::websocket::WsConsole;
			use crate::term::common::ConsoleDriver;
			use crate::term::frontend::types::{UserTerminal, ScriptedTerminal};

			let stream = WsConsole::new(&ctx).await.unwrap();
			let driver = ConsoleDriver::from_connection(ctx, Box::new(stream.fuse()));
			if self.raw {
				todo!("raw stream forwarder");
			} else if std::io::stdin().is_tty() {
				let ut = UserTerminal::new(driver);
				ut.run().await.expect("an error within the tty stdin-driven terminal program");
			} else {
				let st = ScriptedTerminal::new(driver);
				st.run().await.expect("an error within the non-tty stdin-driven terminal program");
			}
		} else {
			todo!("no console device found for descriptor");
		}
/*
		if let Some(uuid) = dest_uuid {
			debug!("attemping to open console connection on {:?} to UUID {:?}", &auth.host, &uuid);
			self.open_terminal(&auth.host, &uuid).await;
		} else {
			eprintln!("Unable to find device by path or invalid UUID: {:?}", dest);
		}
		
		debug!("closed terminal");
*/
		Ok(())
	}
}
