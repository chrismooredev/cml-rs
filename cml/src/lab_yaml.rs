
use serde::{Serialize, Deserialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct LabConfig {
    pub lab: LabDesc,
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct LabDesc {
    pub description: String,
    pub notes: String,
    pub timestamp: f64,
    pub title: String,
    pub version: String,
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub node_definition: String,
    pub x: isize,
    pub y: isize,
    pub configuration: String,
    pub tags: Vec<String>,
    pub interfaces: Vec<Interface>,
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// Link ID
    pub id: String,
    /// Interface on n1 this link is attached to
    pub i1: String,
    /// A device this link is attached to
    pub n1: String,
    /// Interface on n2 this link is attached to
    pub i2: String,
    /// A device this link is attached to
    pub n2: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Interface {
    pub id: String,
    pub slot: Option<isize>, // usize ?
    pub label: String,
    /// If a type is physical, virtual, etc
    #[serde(rename = "type")]
    pub format: String,
}
