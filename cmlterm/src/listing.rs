use clap::Clap;
use log::trace;
use rt::key::Console;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;

use cml::rest::{Authenticate, CmlUser};
use cml::rt;
type CmlResult<T> = Result<T, cml::rest::Error>;

pub struct ListingEntry {
	state: rt::State,
	title: String,
	id: String,
	owner: String,
	nodes: Result<Vec<ListingEntryConsoleNode>, String>,
}
struct ListingEntryConsoleNode {
	state: rt::State,
	label: String,
	id: String,
	lines: Result<Vec<String>, String>,
}

#[derive(Serialize, Deserialize)]
struct ConsoleNodeEntry {
	node_id: String,
	node_title: String,

	lines: Result<Vec<String>, rt::State>,
}
#[derive(Serialize, Deserialize)]
struct ConsoleLabEntry {
	owner: String,
	lab_id: String,
	lab_title: String,

	nodes: Result<Vec<ConsoleNodeEntry>, rt::State>,
}
impl fmt::Display for ConsoleLabEntry {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match &self.nodes {
			Err(state) => writeln!(f, "[{}] /{}/- :: {:?}\t(not available, lab is {:?})", self.owner, self.lab_id, self.lab_title, state)?,
			Ok(nodes) => {
				for node in nodes {
					match &node.lines {
						Err(state) => writeln!(f, "[{}] /{}/{}/- :: {:?}/{}\t(not available, device is {:?})", self.owner, self.lab_id, node.node_id, self.lab_title, node.node_title, state)?,
						Ok(lines) => {
							for (line_id, line) in lines.iter().enumerate() {
								writeln!(f, "[{}] /{}/{}/{} :: {:?}/{}\t({})", self.owner, self.lab_id, node.node_id, line_id, self.lab_title, node.node_title, line)?;
							}
						}
					}
				}
			}
		};
		Ok(())
	}
}
impl fmt::Debug for ConsoleLabEntry {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		writeln!(f, "{}", serde_json::to_string(self).unwrap())
	}
}

struct ListingEntryVNCNode {
	state: rt::State,
	label: String,
	id: String,
	lines: Result<String, String>,
}

#[derive(Clap)]
pub struct SubCmdList {
	/// Shows lines/devices for all users, and all console lines
	#[clap(short, long)]
	all: bool,

	#[clap(short, long)]
	json: bool,

	#[clap(short, long, conflicts_with = "links")]
	vnc: bool,

	#[clap(short, long, conflicts_with = "vnc")]
	links: bool,
}

enum ListType {
	Console,
	VNC,
	Link,
}
struct ListingContext<'a> {
	show_all: bool,
	json: bool,
	listing_type: ListType,
	client: &'a CmlUser,
}
impl<'a> ListingContext<'a> {
	async fn console_lines(&self) -> CmlResult<Vec<ConsoleLabEntry>> {
		// get active keys
		// get active topos
		// group into labs > nodes

		// if show_all
		//    get all labs/topos
		//        insert inactive labs w/ state
		//        insert inactive nodes of labs w/ state
		
		// get the keys
		let keys = self.client.keys_console(self.show_all).await?;

		// group key nodes by (lab_id, node_id)
		let node_lines: HashMap<(&str, &str), Vec<(&Console, &str)>> = {
			let mut nodes: HashMap<(&str, &str), Vec<(&Console, &str)>> = HashMap::new();
			for (uuid, key_meta) in &keys {
				nodes.entry((key_meta.lab_id.as_str(), key_meta.node_id.as_str()))
					.or_default()
					.push((key_meta, uuid.as_str()));
			}
			nodes
		};

		// contains active (and if applicable, inactive) lab topologies (lab + node names)
		let lab_topos: HashMap<String, rt::LabTopology> = {
			// get all node IDs if we are showing all, otherwise only get topos for active labs
			let lab_ids: HashSet<String> = if self.show_all {
				self.client.labs(self.show_all).await?.into_iter().collect()
			} else {
				// could just use /labs/{lab_id} since we'll just need the lab name
				// but getting the topology also lets us (potentially) get inactive nodes
				node_lines.keys().map(|(lid, _)| lid.to_string()).collect()
			};

			let lab_topo_futures: Vec<_> = lab_ids.into_iter()
				.map(async move |lid| {
					self.client.lab_topology(&lid, false).await
						.map(|opt| opt.expect("Lab deleted during execuition. Rerun query."))
						.map(move |topo| (lid, topo))
				})
				.collect();

			let lab_topos: Vec<(String, rt::LabTopology)> = futures::future::join_all(lab_topo_futures).await
				.into_iter()
				.collect::<Result<_, _>>()?;
			
			// convert to HashMap
			lab_topos.into_iter().collect()
		};

		let active_labs: HashSet<&str> = keys.iter().map(|(_, meta)| meta.lab_id.as_str()).collect();
		if self.show_all {
			let mut inactive_labs: HashSet<&str> = HashSet::new();
			for k in lab_topos.keys() {
				if ! active_labs.contains(k.as_str()) {
					inactive_labs.insert(k);
				}
			}
			Some(inactive_labs)
		} else { None };

		let mut node_lines = node_lines;
		let mut lab_entries: Vec<ConsoleLabEntry> = Vec::new();

		for (lid, topo) in lab_topos.iter() {
			let nodes: Result<Vec<ConsoleNodeEntry>, rt::State> = if active_labs.contains(lid.as_str()) {
				// if self.show_all then entry for each node
				//    else, only an entry for each active node
				
				let nodes: Vec<ConsoleNodeEntry> = if self.show_all {
					topo.nodes.iter()
						.map(|node| {
							let lines: Result<Vec<String>, rt::State> = node_lines.remove(&(lid.as_str(), node.id.as_str()))
								.ok_or(node.data.state)
								.map(|mut lines| {
									lines.sort_by_key(|(meta, _)| meta.line);
									lines.into_iter()
										.map(|(_, uuid)| uuid.to_string())
										.collect()
								});

							ConsoleNodeEntry {
								node_id: node.id.clone(),
								node_title: node.data.label.clone(),
								lines,
							}
						})
						.collect()
				} else {
					topo.nodes.iter()
						.filter_map(|node| node_lines.get(&(lid.as_str(), node.id.as_str())).map(|lines| (node, lines)))
						.map(|(node, lines)| {
							let mut lines = lines.clone();
							lines.sort_by_key(|(meta, _)| meta.line);
							// assume there aren't non-contiguous lines
							let lines = Ok(lines.iter().map(|(_, uuid)| uuid.to_string()).collect());

							ConsoleNodeEntry {
								node_id: node.id.clone(),
								node_title: node.data.label.clone(),
								lines
							}
						})
						.collect()
				};

				Ok(nodes)
			} else {
				Err(topo.state)
			};

			lab_entries.push(ConsoleLabEntry {
				owner: topo.owner.clone(),
				lab_id: lid.clone(),
				lab_title: topo.title.clone(),
			
				nodes,
			});
		}

		return Ok(lab_entries);
	}

	fn print_data<T: fmt::Debug + fmt::Display>(&self, data: &[T]) {
		for l in data {
			if self.json {
				print!("{:?}", l);
			} else {
				print!("{}", l);
			}
		}
	}
}

impl SubCmdList {
	pub async fn run(&self, auth: &Authenticate) -> CmlResult<()> {
		trace!("logging in");
		let client = auth.login().await?;
		let client: &CmlUser = &client;
		trace!("listing data");

		let ctx = self.context(&client);
		match ctx.listing_type {
			ListType::Console => {
				let lines = ctx.console_lines().await?;
				ctx.print_data(&lines);
			},
			ListType::VNC => {
				todo!("vnc");
			},
			ListType::Link => {
				todo!("device links");
			},
		}

		Ok(())
	}
	
	fn context<'a>(&'a self, client: &'a CmlUser) -> ListingContext<'a> {
		let listing_type = match (self.vnc, self.links) {
			(true, true) => panic!("Not possible because of clap argument parsing conflicts"),
			(true, false) => ListType::VNC,
			(false, true) => ListType::Link,
			(false, false) => ListType::Console,
		};

		ListingContext {
			show_all: self.all,
			json: self.json,
			listing_type,
			client,
		}
	}

	pub fn list_data(&self, mut lab_entries: Vec<ListingEntry>) {
		trace!("listing data");
		lab_entries.sort_by_cached_key(|asd| (asd.state, asd.owner.clone(), asd.title.clone(), asd.id.clone()));

		for lab in lab_entries {
			match lab.nodes {
				Err(_) => if self.all { println!("[{}] /{} :: '{}' (no devices available, lab state is {:?})", lab.owner, lab.id, lab.title, lab.state) },
				Ok(mut nodes) => {
					
					// sort booted stuff first, then by label
					nodes.sort_by_cached_key(|list_nodes| (list_nodes.state, list_nodes.label.clone(), list_nodes.id.clone()));

					for node in nodes {
						match node.lines {
							Err(_) => if self.all {
								println!("[{}] /{}/{} :: '{}'/{} (no lines available, device state is {:?})", lab.owner, lab.id, node.id, lab.title, node.label, node.state)
							},
							Ok(lines) => {
								for (line, line_guid) in lines.iter().enumerate()
									// if not self.all, only display first
									.take_while(|(i, _)| if self.all { true } else { *i == 0 })
								{
									println!("[{}] /{}/{}/{} :: '{}'/{}  ({})", lab.owner, lab.id, node.id, line, lab.title, node.label, line_guid);
								}
							}
						}
					}
				}
			}
		}

		/*

		[user] <lab title> <node label> :: <lab ID> <node ID> <lines>
		<lab title> <node label> :: <lab ID> <node ID> <lines> (not available, <reason>)

		<lab title> (not available, <reason>)

		*/
	}
}
