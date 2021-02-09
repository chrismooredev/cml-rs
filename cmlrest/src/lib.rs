
#![feature(str_split_once)]

use clap::Clap;
use serde_json::Value;
use cml::rest::{CmlUser, Error as RestError};

#[derive(Clap)]
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
		}
	}
}

#[derive(Clap)]
pub enum SubCmdRoot {
	Test,
	Get(SubCmdGet),
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
