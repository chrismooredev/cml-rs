
use std::collections::HashMap;

use log::{debug, error, trace, warn};
use thiserror::Error;
use cml::rest::CmlUser;

use cml::rest_types::LabTopology;
use cml::rest_types as rt;
type CmlResult<T> = Result<T, cml::rest::Error>;

#[derive(Debug, Clone, Copy, Error)]
pub enum NodeSearchError {
	#[error("No lab matching the desired lab descriptor")]
	NoMatchingLab,
	#[error("No node matching the desired node descriptor within the specified lab")]
	NoMatchingNode,
	#[error("Multiple labs matched by lab title. Try renaming the labs, or using a lab ID")]
	MultipleMatchingLabs,
	#[error("Multiple nodes matched by node title within the lab. Try renaming the nodes, or using a node ID")]
	MultipleMatchingNodes,
}
#[derive(Debug, Clone, Copy, Error)]
pub enum ConsoleSearchError {
	/// Unable to find a device with the specified lab/node descriptors
	#[error("Unable to resolve lab/node descriptor")]
	BadDeviceQuery(#[from] NodeSearchError),
	/// Node does not have any active lines for the specified line number
	#[error("Node does not have specified line")]
	NoMatchingLines,
	/// Node is not currently active
	#[error("Specified node is not currently active")]
	NodeNotActive,
	/// Node does not provide console lines
	#[error("Specified node does not provide console lines")]
	Unsupported,
}

#[derive(Debug, Clone)]
pub struct NodeCtx {
	host: String,
	user: String,
	lab: (String, String),
	node: (String, String),
	meta: rt::NodeDescription,
}
impl NodeCtx {
	pub fn host(&self) -> &str { &self.host }
	pub fn user(&self) -> &str { &self.user }
	pub fn lab(&self) -> (&str, &str) { (&self.lab.0, &self.lab.1) }
	pub fn node(&self) -> (&str, &str) { (&self.node.0, &self.node.1) }
	pub fn meta(&self) -> &rt::NodeDescription { &self.meta }


	pub async fn search(client: &CmlUser, lab: &str, device: &str) -> CmlResult<Result<NodeCtx, NodeSearchError>> {
		// get all the labs and topologies on the CML server
		let labs = client.labs(true).await?;
		let topos = client.lab_topologies(&labs, false).await?
			.into_iter()
			.map(|(s, t)| (s.to_string(), t.expect("Lab removed during searching")))
			.collect::<HashMap<_, _>>();

		// find an appropriate lab, prioritizing IDs on conflicts
		// returns early if none are found or found multiple labs with desired title
		let (lab_id, lab_topo) = match <(String, LabTopology)>::find_single(topos.into_iter(), lab) {
			Err(se) => return Ok(Err(se)),
			Ok(s) => s
		};
		let (node_id, node_data) = match <rt::labeled::Data<rt::NodeDescription>>::find_single(lab_topo.nodes, device) {
			Err(se) => return Ok(Err(se)),
			Ok(node) => (node.id, node.data)
		};

		Ok(Ok(NodeCtx {
			host: client.host().to_owned(),
			user: client.username().to_owned(),
			lab: (lab_id, lab_topo.title),
			node: (node_id, node_data.label.clone()),
			meta: node_data,
		}))
	}
	pub async fn resolve_line(self, client: &CmlUser, line: Option<u64>, keys: Option<&HashMap<String, rt::key::Console>>) -> CmlResult<Result<ConsoleCtx, ConsoleSearchError>> {
		ConsoleCtx::search(client, self, line, keys).await
	}
}

// note: we want to control these fields so data cannot go out of sync
#[derive(Debug, Clone)]
pub struct ConsoleCtx {
	node: NodeCtx,
	line: u64,
	uuid: String,
}
impl ConsoleCtx {
	/*async fn init_console(self) -> Result<(), ()> {
		Console::from_ctx(self).await
	}*/

	pub fn node(&self) -> &NodeCtx { &self.node }
	pub fn line(&self) -> &u64 { &self.line }
	pub fn uuid(&self) -> &str { &self.uuid }

	/// Searches a CML instance for a valid line
	pub async fn search(client: &CmlUser, node: NodeCtx, line: Option<u64>, keys: Option<&HashMap<String, rt::key::Console>>) -> CmlResult<Result<ConsoleCtx, ConsoleSearchError>> {
		// TODO: allow filtering by CML username?

		debug!("resolved node: {:?}", node);
		
		let mut owned_keys: Option<HashMap<String, rt::key::Console>> = None;

		let keys = match keys {
			None => {
				owned_keys = Some(client.keys_console(true).await?);
				owned_keys.as_ref().unwrap()
			},
			Some(k) => k,
		};

		let mut node_lines = keys.into_iter()
			.filter(|(_uuid, meta)| meta.lab_id == node.lab.0)
			.filter(|(_uuid, meta)| meta.node_id == node.node.0)
			.filter(|(_uuid, meta)| line.map(|line| meta.line == line).unwrap_or(true))
			.collect::<Vec<(&String, &rt::key::Console)>>();

		node_lines.sort_by_key(|(_, meta)| meta.line);

		match node_lines.len() {
			0 => { // 0
				if ! node.meta.state.active() {
					Ok(Err(ConsoleSearchError::NodeNotActive))
				} else {
					// check if it actually supports a console line
					let node_defs = client.simplified_node_definitions().await?;
					let node_def = node_defs.into_iter()
						.find(|nd| nd.id == node.meta.node_definition)
						.expect("Device uses unregistered node definition");
					//node_def.data.device.interfaces.serial_ports
	
					if ! node_def.data.sim.console {
						// This device does not support console lines
						Ok(Err(ConsoleSearchError::Unsupported))
					} else {
						assert!(line.is_some());
						Ok(Err(ConsoleSearchError::NoMatchingLines))
					}
				}
			},
			_ => { // 1+
				let (uuid, meta) = node_lines.remove(0);
	
				// if a line was specified, then all our lines were filtered to it
				// if no line was specified, then we use the lowest line number (we sorted above)
	
				Ok(Ok(ConsoleCtx {
					node,
					line: meta.line,
					uuid: uuid.to_owned(),
				}))
			}
		}
	}

}

// internal helpers used for ConsoleCtx::search
trait NamedMatcher {
	fn matcher_kind() -> &'static str;
	fn none_found() -> NodeSearchError;
	fn multiple_found() -> NodeSearchError;
	fn id(&self) -> &str;
	fn label(&self) -> &str;
	fn state(&self) -> rt::State;
	
	fn matches(&self, descriptor: &str) -> bool {
		self.id() == descriptor || self.label() == descriptor
	}

	fn find_single<'a, N: NamedMatcher + Sized, I: IntoIterator<Item = N>>(s: I, desc: &'_ str) -> Result<N, NodeSearchError> {
		let mut matching: Vec<N> = s.into_iter()
			.filter(|lab_data| lab_data.matches(desc))
			.inspect(|n| debug!("found {} matching the provided description: (id, name, state) = {:?}", N::matcher_kind(), (n.id(), n.label(), n.state())))
			.collect();

		if matching.len() == 0 {
			Err(Self::none_found())
			//Err(format!("No {} found by ID/name: {:?}", N::matcher_kind(), desc))
		} else if matching.len() == 1 {
			Ok(matching.remove(0))
		} else {
			let by_id = matching.iter()
				.position(|n| n.id() == desc);

			if let Some(res_i) = by_id {
				debug!("Found multiple {}s for {} description {:?} - interpreting it as an ID", N::matcher_kind(), N::matcher_kind(), desc);
				Ok(matching.remove(res_i))
			} else {
				Err(Self::multiple_found())
				/*let mut s = String::new();
				s += &format!("Found multiple {}s by that description: please reference one by ID, or give them unique names.\n", N::matcher_kind());
				matching.iter().for_each(|n| {
					s += &format!("\t ID = {}, title = {:?} (state: {})\n", n.id(), n.label(), n.state());
				});
				Err(s)*/
			}
		}
	}
}
impl NamedMatcher for (String, LabTopology) {
	fn matcher_kind() -> &'static str { "lab" }
	fn none_found() -> NodeSearchError { NodeSearchError::NoMatchingLab }
	fn multiple_found() -> NodeSearchError { NodeSearchError::MultipleMatchingLabs }
	fn id(&self) -> &str { &self.0 }
	fn label(&self) -> &str { &self.1.title }
	fn state(&self) -> rt::State { self.1.state }
}
impl NamedMatcher for rt::labeled::Data<rt::NodeDescription> {
	fn matcher_kind() -> &'static str { "node" }
	fn none_found() -> NodeSearchError { NodeSearchError::NoMatchingNode }
	fn multiple_found() -> NodeSearchError { NodeSearchError::MultipleMatchingNodes }
	fn id(&self) -> &str { &self.id }
	fn label(&self) -> &str { &self.data.label }
	fn state(&self) -> rt::State { self.data.state }
}
