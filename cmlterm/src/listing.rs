
use std::time::Instant;
use std::collections::HashMap;
use log::{error, warn, info, debug, trace};
use clap::Clap;

use cml::rest::{Authenticate, CmlUser};
use cml::rt;
use cml::rt::LabTopology;
type CmlResult<T> = Result<T, cml::rest::Error>;

pub struct ListingEntry {
	state: rt::State,
	title: String,
	id: String,
	owner: String,
	nodes: Result<Vec<ListingEntryNode>, String>,
}
struct ListingEntryNode {
	state: rt::State,
	label: String,
	id: String,
	lines: Result<Vec<String>, String>,
}

#[derive(Clap)]
pub struct SubCmdList {
	/// Shows all console lines, not just line 0
	#[clap(short, long)]
	all: bool,
}
impl SubCmdList {
	pub async fn run(&self, auth: &Authenticate) -> CmlResult<()> {
		trace!("logging in");
		let client = auth.login().await?;
		let client: &CmlUser = &client;
		trace!("listing data");

		let lab_entries = SubCmdList::get_node_listing_data(&client).await?;
		self.list_data(lab_entries);

		Ok(())
	}
	
	async fn get_node_listing_data(client: &CmlUser) -> CmlResult<Vec<ListingEntry>> {
		//use futures::prelude::*;
		//use futures::{FutureExt, TryFutureExt};

		async fn get_node_topo(client: &CmlUser, id: &str) -> CmlResult<LabTopology> {
			match client.lab_topology(&id, false).await {
				Ok(Some(topo)) => Ok(topo),
				Ok(None) => panic!("Lab was removed during program execution"),
				Err(e) => Err(e),
			}
		}

		let labs = client.labs(true).await?;
		let lab_data_future: _ = labs.iter().map(|lab_id| client.lab(lab_id));
		
		let lab_data: Vec<cml::rt::Lab> = futures::future::join_all(lab_data_future)
			//.unit_error().boxed().compat()
			.await.into_iter() // wait for all to finish
			.filter_map(|lab| lab.transpose()) // filter out None's - there shouldn't be any
			.collect::<Result<Vec<_>, _>>()?;

		// HashMap<(lab_id, node_id), Vec<String>>
		let mut lab_keys_by_key: Vec<(String, String, u64, String)> = client.keys_console(true).await?
			.into_iter()
			.map(|(key, meta)| (meta.lab_id, meta.node_id, meta.line, key))
			.collect();
		lab_keys_by_key.sort();

		let mut lab_keys: HashMap<(String, String), Vec<String>> = HashMap::new();

		lab_keys_by_key.into_iter()
			.for_each(|(lid, nid, line, key)| {
				let p = lab_keys.entry((lid, nid))
					.or_insert(Vec::new());
				assert_eq!(p.len(), line as usize);
				p.push(key);
			});

		let lab_topo_futures = lab_data.iter()
			.map(|s| s.id.as_str())
			.map(|id| get_node_topo(&client, id))/* async move |id| match client.lab_topology(&id, false).await {
				Ok(Some(topo)) => Ok(topo),
				Ok(None) => panic!("Lab was removed during program execution"),
				Err(e) => Err(e),
			})*/;
		let lab_topos = futures::future::join_all(lab_topo_futures)
			.await.into_iter()
			.collect::<CmlResult<Vec<rt::LabTopology>>>()?;

		trace!("organizing data");

		let lab_entries: Vec<ListingEntry> = lab_data.into_iter().zip(lab_topos.into_iter())
			.map(|(meta, topo)| {
				// (node_id, node_data, lines)

				let nodes: Result<Vec<ListingEntryNode>, String> = match meta.state {
					rt::State::Started => {
						let nodes: Vec<ListingEntryNode> = topo.nodes.into_iter()
							.map(|ld| (lab_keys.remove(&(meta.id.clone(), ld.id.clone())), ld.id, ld.data))
							.map(|(keys, id, desc)| {
								let lines = keys.ok_or_else(|| format!("no lines available, device state is {:?}", desc.state));

								ListingEntryNode {
									label: desc.label,
									id,
									state: desc.state,
									lines: lines.map(|v| v.iter().map(|s| s.to_string()).collect()),
								}
							})
							.collect();
						
						Ok(nodes)
					},
					_ => {
						// at least... no nodes SHOULD be available
						Err(format!("no devices available, lab state is {:?}", meta.state))
					}
				};

				ListingEntry {
					state: meta.state,
					owner: meta.owner,
					title: meta.title,
					id: meta.id,
					nodes,
				}
			})
			.collect();

		Ok(lab_entries)
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

