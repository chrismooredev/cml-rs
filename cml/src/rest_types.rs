use serde::{Deserialize, Serialize};
use serde_with::{SerializeDisplay, DeserializeFromStr};
use std::fmt;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "schemars", schemars(rename_all = "snake_case"))]
pub enum State {
	/// The device/lab has been fully booted and is currently running.
	Booted,
	/// The device/lab is partially booted, or in the process of starting up.
	Started,
	/// The device is in a queue to be started up.
	Queued,
	/// The device is currently being stopped.
	Stopped,
	/// The device is not running, or queued to run.
	DefinedOnCore,
	DefinedOnCluster,
}
impl State {
	pub fn active(&self) -> bool {
		matches!(self, State::Booted | State::Started | State::Queued)
	}
	pub fn inactive(&self) -> bool {
		matches!(self, State::DefinedOnCluster | State::DefinedOnCore | State::Stopped)
	}
}
impl fmt::Display for State {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(match self {
			State::Booted => "Booted",
			State::Started => "Started",
			State::Stopped => "Stopped",
			State::Queued => "Queued",
			State::DefinedOnCore => "Defined on Core",
			State::DefinedOnCluster => "Defiend on Cluster",
		})
	}
}


#[derive(Debug, SerializeDisplay, DeserializeFromStr, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeDefinition {
	ASAv,
	IOSv,
	IOSvL2,
	IOSxrv,
	IOSxrv9000,
	NXosv,
	NXosv9000,
	CSR1000v,
	UnmanagedSwitch,
	ExternalConnector,
	WANEmulator,
	Alpine, TRex, CoreOS, Ubuntu,
	Server, Desktop,

	/// An unknown node definition.
	Unknown(String)
}
impl NodeDefinition {
	const ID_MAPPING: &'static [(&'static str, NodeDefinition)] = &[
		("asav",               NodeDefinition::ASAv),
		("iosv",               NodeDefinition::IOSv),
		("iosvl2",             NodeDefinition::IOSvL2),
		("iosxrv",             NodeDefinition::IOSxrv),
		("iosxrv9000",         NodeDefinition::IOSxrv9000),
		("nxosv",              NodeDefinition::NXosv),
		("nxosv9000",          NodeDefinition::NXosv9000),
		("csr1000v",           NodeDefinition::CSR1000v),
		("unmanaged_switch",   NodeDefinition::UnmanagedSwitch),
		("external_connector", NodeDefinition::ExternalConnector),
		("wan_emulator",       NodeDefinition::WANEmulator),
		("alpine",             NodeDefinition::Alpine),
		("trex",               NodeDefinition::TRex),
		("coreos",             NodeDefinition::CoreOS),
		("ubuntu",             NodeDefinition::Ubuntu),
		("server",             NodeDefinition::Server),
		("desktop",            NodeDefinition::Desktop),
	];

	pub fn is_ios(&self) -> bool {
		use NodeDefinition::*;
		matches!(self, IOSv | IOSvL2 | IOSxrv | IOSxrv9000)
	}
	pub fn is_asa(&self) -> bool {
		use NodeDefinition::*;
		matches!(self, ASAv)
	}
	pub fn is_linux(&self) -> bool {
		use NodeDefinition::*;
		matches!(self, Alpine | TRex | CoreOS | Ubuntu | Server | Desktop)
	}
	pub fn is_unknown(&self) -> Option<&str> {
		use NodeDefinition::*;
		match self {
			Unknown(ty) => Some(&ty),
			_ => None
		}
	}
}

impl std::str::FromStr for NodeDefinition {
	type Err = std::convert::Infallible;
	fn from_str(s: &str) -> Result<NodeDefinition, Self::Err> {
		for (cml_id, variant) in NodeDefinition::ID_MAPPING {
			if s == *cml_id {
				return Ok(variant.clone());
			}
		}
		Ok(NodeDefinition::Unknown(s.to_string()))
	}
}
impl fmt::Display for NodeDefinition {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		if let NodeDefinition::Unknown(unk) = self {
			f.write_str(&unk)
		} else {
			let label = NodeDefinition::ID_MAPPING.iter()
				.find(|(_, v)| self == v)
				.map(|(l, _)| l)
				.expect("missing node definition mapping");
			f.write_str(label)
		}
	}
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lab {
	pub id: String,
	pub state: State,
	pub created: String,
	#[serde(rename = "lab_title")]
	pub title: String,
	pub owner: String,
	#[serde(rename = "lab_description")]
	pub description: String,
	pub node_count: isize,
	pub link_count: isize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleNode {
	pub id: String,
	pub label: String,
	pub x: isize,
	pub y: isize,
	pub node_definition: NodeDefinition,
	pub image_definition: Option<String>,
	pub state: State,

	// not really sure what many of these types actually are... The jagger API browser just shows 'null'
	pub cpus: Option<isize>,
	pub cpu_limit: Option<isize>,
	pub ram: Option<isize>,
	pub data_volume: Option<String>,
	pub boot_disk_size: Option<isize>,
	pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabTopology {
	#[serde(rename = "lab_title")]
	pub title: String,
	#[serde(rename = "lab_notes")]
	pub notes: String,
	#[serde(rename = "lab_description")]
	pub description: String,
	#[serde(rename = "lab_owner")]
	pub owner: String,
	pub state: State,
	pub created_timestamp: f64,
	pub cluster_id: Option<String>,
	pub version: String,

	pub nodes: Vec<labeled::Data<NodeDescription>>,
	pub links: Vec<labeled::Link<LinkDescription>>,
	pub interfaces: Vec<labeled::Interface<InterfaceDescription>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDescription {
	pub x: isize,
	pub y: isize,
	pub label: String,
	pub node_definition: NodeDefinition,
	pub image_definition: Option<String>,
	pub state: State,
	pub configuration: Option<String>,

	pub cpus: Option<isize>,
	pub cpu_limit: Option<isize>,
	pub ram: Option<isize>,
	pub data_volume: Option<String>,
	pub boot_disk_size: Option<isize>,
	pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkDescription {
	//pub src_int: String,
	//pub dst_int: String,
	pub state: State,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceDescription {
	/// physical, loopback, etc
	#[serde(rename = "type")]
	pub mode: String,
	// pub node: String, // shows on example, but not on any topology calls
	#[serde(rename = "label")]
	pub int_label: String,
	pub slot: Option<u64>,
	pub state: State,
}

pub mod key {
	use serde::{Deserialize, Serialize};

	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct Console {
		pub lab_id: String,
		pub node_id: String,
		#[serde(rename = "label")]
		pub node_label: String,
		pub line: u64,
	}

	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct VNC {
		pub lab_id: String,
		pub node_id: String,
		#[serde(rename = "label")]
		pub node_label: String,
	}

	// for consistency's sake
	pub type Link = String;
}

pub mod labeled {
	use serde::{Deserialize, Serialize};

	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct Data<T> {
		pub id: String,
		pub data: T,
	}

	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct Link<T> {
		pub id: String,
		pub interface_a: String,
		pub interface_b: String,
		pub data: T,
	}

	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct Interface<T> {
		#[serde(rename = "id")]
		pub int_id: String,
		#[serde(rename = "node")]
		pub node_id: String,
		pub data: T,
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleNodeDefinition {
	pub id: NodeDefinition,
	pub image_definitions: Vec<String>,
	pub data: SimpleNodeDefData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleNodeDefData {
	pub general: NodeDefData::General,
	pub device: NodeDefData::Device,
	pub ui: NodeDefData::UI,
	pub sim: NodeDefData::Sim,
}
mod NodeDefData {
	use super::*;

	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct General {
		pub description: String,
		pub nature: String,
		pub read_only: bool,
	}

	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct Device {
		pub interfaces: Interfaces,
	}
	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct Interfaces {
		pub has_loopback_zero: bool,
		pub default_count: usize,
		pub physical: Vec<String>,
		pub serial_ports: usize,
	}

	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct UI {
		pub description: String,
		pub group: String,
		pub icon: String,
		pub label: String,
		pub label_prefix: String,
		pub visible: bool,
		pub has_configuration: bool,
		pub show_ram: bool,
		pub show_cpus: bool,
		pub show_cpu_limit: bool,
		pub show_data_volume: bool,
		pub show_boot_disk_size: bool,
		pub has_config_extraction: bool,
	}

	#[derive(Debug, Clone, Serialize, Deserialize)]
	pub struct Sim {
		pub ram: Option<usize>,
		pub cpus: Option<usize>,
		pub cpu_limit: Option<usize>,
		pub data_volume: Option<usize>,
		pub boot_disk_size: Option<usize>,
		pub console: bool,
		pub simulate: bool,
		pub vnc: bool,
	}
}
