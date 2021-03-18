
use clap::Clap;
use log::trace;

type CmlResult<T> = Result<T, cml::rest::Error>;

use cmlterm::expose::SubCmdExpose;
use cmlterm::listing::SubCmdList;
use cmlterm::open_term::SubCmdOpen;

#[derive(Clap)]
#[clap(version = clap::crate_version!(), author = "Chris M. <35407569+csm123199@users.noreply.github.com>")]
struct Args {
	#[clap(subcommand)]
	subc: SubCmd,
}

#[derive(Clap)]
enum SubCmd {
	List(SubCmdList),
	Open(SubCmdOpen),
	Expose(SubCmdExpose),
	Run(SubCmdRun),
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
		SubCmd::List(list) => list.run(&auth).await?,
		SubCmd::Open(open) => open.run(&auth).await?,
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
	cmlterm [--vnc] open /<LAB_NAME>/<DEVICE_NAME>[/LINE_NUMBER = 0]
	cmlterm [--vnc] open /<LAB_ID>/<DEVICE_ID>[/LINE_NUMBER = 0]

	// opens a port for other applications to connect to interact the the device like telnet
	cmlterm [--vnc] expose [--json] <UUID>
	cmlterm [--vnc] expose [--json] /<LAB_NAME>/<DEVICE_NAME>[/LINE_NUMBER = 0] // prints {"host": "localhost", "port": XXXXX}
	cmlterm [--vnc] expose [--json] /<LAB_ID>/<DEVICE_ID>[/LINE_NUMBER = 0] // prints {"host": "localhost", "port": XXXXX}

	// runs a sequence of commands from arguments, or stdin, to run on the device
	cmlterm run <UUID>
	cmlterm run /<LAB_NAME>/<DEVICE_NAME>[/LINE_NUMBER = 0] [COMMAND, ...] (or over stdin)
	cmlterm run /<LAB_ID>/<DEVICE_ID>[/LINE_NUMBER = 0] [COMMAND, ...] (or over stdin)
*/
