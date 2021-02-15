use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
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

#[derive(Debug, Serialize, Deserialize)]
pub struct SimpleNode {
	pub id: String,
	pub label: String,
	pub x: isize,
	pub y: isize,
	pub node_definition: String,
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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeDescription {
	pub x: isize,
	pub y: isize,
	pub label: String,
	pub node_definition: String,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct LinkDescription {
	//pub src_int: String,
	//pub dst_int: String,
	pub state: State,
}

#[derive(Debug, Serialize, Deserialize)]
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

	#[derive(Serialize, Deserialize)]
	pub struct Console {
		pub lab_id: String,
		pub node_id: String,
		#[serde(rename = "label")]
		pub node_label: String,
		pub line: u64,
	}

	#[derive(Serialize, Deserialize)]
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

	#[derive(Debug, Serialize, Deserialize)]
	pub struct Data<T> {
		pub id: String,
		pub data: T,
	}

	#[derive(Debug, Serialize, Deserialize)]
	pub struct Link<T> {
		pub id: String,
		pub interface_a: String,
		pub interface_b: String,
		pub data: T,
	}

	#[derive(Debug, Serialize, Deserialize)]
	pub struct Interface<T> {
		#[serde(rename = "id")]
		pub int_id: String,
		#[serde(rename = "node")]
		pub node_id: String,
		pub data: T,
	}
}
