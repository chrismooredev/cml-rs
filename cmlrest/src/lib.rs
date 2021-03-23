
use clap::Clap;
use cml::rest::{CmlUser, Error as RestError};
use serde_json::Value;

#[derive(Clap)]
#[clap(version=clap::crate_version!())]
pub struct Args {
	/// Emit the raw JSON response
	#[clap(short)]
	json: bool,

	#[clap(short = 'a')]
	show_all: bool,

	#[clap(subcommand)]
	rootsubcmd: SubCmdRoot,
}
impl Args {
	pub async fn handle(&self, client: &CmlUser) -> Result<(), RestError> {
		match &self.rootsubcmd {
			SubCmdRoot::Test => testing(&self, &client).await,
			SubCmdRoot::Get(g) => g.handle(&self, &client).await,
			SubCmdRoot::Raw(g) => g.handle(&self, &client).await,
		}
	}
}

#[derive(Clap)]
pub enum SubCmdRoot {
	Test,
	Get(SubCmdGet),
	Raw(SubCmdRaw),
}

#[derive(Clap)]
pub enum SubCmdGet {
	Labs,
	Lab(LabID),
}
impl SubCmdGet {
	async fn handle(&self, args: &Args, client: &CmlUser) -> Result<(), RestError> {
		match self {
			SubCmdGet::Labs => {
				let result = client.labs(args.show_all).await?;
				let output = if args.json {
					serde_json::to_string(&result)?
				} else {
					result.join("\n")
				};
				println!("{}", output);
			},
			SubCmdGet::Lab(LabID { arg }) => {
				let arg = arg.trim();
				let result = client.lab(arg).await?;
				match (args.json, result) {
					(true, Some(l)) => println!("{}", serde_json::to_string(&l)?),
					(true, None) => println!("{}", serde_json::to_string(&Value::Null)?),
					(false, None) => println!("No lab found with ID {}", arg),
					(false, Some(l)) => {
						println!("Lab {}, \"{}\" ({})", l.id, l.title, l.state);
						println!("Owner: {}, Created: {}", l.owner, l.created);
						println!("{} nodes, {} links", l.node_count, l.link_count);
						if l.description.trim().len() > 0 {
							println!("Description:");
							termimad::print_text(l.description.trim());
						}
					},
				}
			}
		}

		Ok(())
	}
}

use clap::ArgEnum;
#[derive(ArgEnum)]
enum HttpMethod {
	GET,
	PUT,
}

impl ::std::str::FromStr for HttpMethod {
	type Err = String;

	fn from_str(s: &str) -> ::std::result::Result<Self,Self::Err> {
		// roughly equivalent to clap 2.33.3 ArgEnum derive impl
		
		#[allow(deprecated, unused_imports)]
		use ::std::ascii::AsciiExt;
		if s.eq_ignore_ascii_case("GET") {
			Ok(Self::GET)
		} else if s.eq_ignore_ascii_case("PUT") {
			Ok(Self::PUT)
		} else {
			Err(format!("valid values: {}", ["get", "put"].join(", ")))
		}
	}
}


#[derive(Clap)]
pub struct SubCmdRaw {
	#[clap(case_insensitive=true)]
	method: HttpMethod,
	path: String,
}
impl SubCmdRaw {
	async fn handle(&self, args: &Args, client: &CmlUser) -> Result<(), RestError> {
		let rresp = match self.method {
			HttpMethod::GET => {
				// make request, print to stdout
				let resp = cml::rest::raw::get_v0(client, self.path.clone())
					.send().await?;
				
				cml::rest::RawApiResponse::extract(resp).await?
			},
			HttpMethod::PUT => {
				use tokio::io::AsyncReadExt;

				// make request, pipe stdin to request, print to stdout
				let mut stdin = tokio::io::stdin();
				let mut buf = Vec::with_capacity(1024);
				stdin.read_to_end(&mut buf).await?;
				
				let resp = cml::rest::raw::put_v0(client, self.path.clone())
					.body(buf)
					.send().await?;
				
				cml::rest::RawApiResponse::extract(resp).await?
			},
		};

		match rresp {
			(200, val) => {
				println!("{}", val.as_string());
			},
			(_, val) => {
				eprintln!("{}", val.as_string());
			}
		}

		Ok(())
	}
}

#[derive(Clap)]
pub struct LabID {
	arg: String,
}

async fn testing(_args: &Args, client: &CmlUser) -> Result<(), RestError> {
	let labs = client.labs(true).await?;
	println!("labs: {:?}", labs);

	for lab_id in &labs {
		//println!("Lab ID: {}", lab_id);
		println!("{:#?}", client.lab(&lab_id).await?.unwrap());
	}

	Ok(())
}
