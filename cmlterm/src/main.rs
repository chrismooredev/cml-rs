
use clap::Clap;
use log::trace;

use cmlterm::TerminalError;
use cmlterm::expose::SubCmdExpose;
use cmlterm::listing::SubCmdList;
use cmlterm::open_term::SubCmdOpen;

#[derive(Clap)]
#[clap(version = clap::crate_version!(), author = "Chris M. <35407569+csm123199@users.noreply.github.com>")]
struct Args {
	/// Emit a source-able completions script for the specified shell.
	#[clap(long)]
	completions: Option<String>,

	#[clap(subcommand)]
	subc: Option<SubCmd>,
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

async fn application() -> anyhow::Result<()> {
	env_logger::init();

	trace!("parsing args");
	// can eventually 'execve ssh' ?
	let args = Args::parse();

	if let Some(shell) = args.completions {
		let script: &str = match shell.to_lowercase().as_str() {
			"bash" => include_str!("../__cmlterm_completion_wrapper.sh"),
			_ => anyhow::bail!(TerminalError::UnsupportedCompletionShell(shell)),
		};
		
		print!("{}", script);
		return Ok(());
	}

	if args.subc.is_none() {
		use clap::IntoApp;
		use std::io;

		let mut app = Args::into_app();
		let mut out = io::stdout();
		app.write_help(&mut out).expect("failed to write to stdout");
		return Ok(());
	}

	let auth = cml::get_auth_env().expect("Unable to get authentication info from environment");

	match &args.subc.unwrap() {
		SubCmd::List(list) => list.run(&auth).await?,
		SubCmd::Open(open) => open.run(&auth).await?,
		SubCmd::Run(_run) => todo!("running individial/batch commands non-interactively"),
		SubCmd::Expose(expose) => expose.run(&auth.host).await,
	}

	Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
	application().await
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
