
use clap::Clap;
use cml::get_auth_env;
use cml::rest::{Error as RestError};

use cmlrest::Args;

#[tokio::main]
async fn main() -> Result<(), RestError> {
	let args: Args = Args::parse();
	
	let auth_creds = get_auth_env().unwrap();
	let client = auth_creds.login().await?;

	args.handle(&client).await
}
