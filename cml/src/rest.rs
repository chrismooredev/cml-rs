use std::borrow::Cow;
use reqwest::{header::HeaderMap, Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

use rt::SimpleNode;
type RResult<T> = Result<T, CmlError>;
pub use CmlError as Error;

#[derive(Debug, Error)]
pub enum CmlError {
	#[error("stdio error")]
	Io(#[from] std::io::Error),
	#[error("network error")]
	Network(#[from] reqwest::Error),
	#[error("bad response from CML REST API")]
	Response(#[from] ApiError),
	#[error("error decoding JSON response")]
	Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
#[error("There was an error invoking the CML REST API for {} ({:?})", endpoint, error_type)]
pub struct ApiError {
	endpoint: String,
	error_type: ApiErrorType,
}
impl ApiError {
	fn new(endpoint: impl Into<String>, error_type: ApiErrorType) -> ApiError {
		ApiError {
			endpoint: endpoint.into(),
			error_type,
		}
	}
}

#[derive(Debug)]
enum ApiErrorType {
	/// Catch-all
	Unknown(String),

	/// Got a bad response from the server
	BadResponse(String, String),

	/// Error decoding a JSON response. Contains erroring JSON, as well as decoding error.
	JsonDecode(String, String, serde_json::Error),

	// - /authenticate
	// - /licensing/tech_support
	AuthenticationFailure,
}
impl ApiErrorType {
	fn unknown<S: Into<String>>(s: S) -> ApiErrorType {
		ApiErrorType::Unknown(s.into())
	}
}

#[derive(Debug, Clone, PartialEq)]
pub enum RawApiResponse {
	None,
	PlainText(String),
	Json(Value),
}
/*impl RawApiResponse {
	/// Attempts to return the contained value as the generic type. Otherwise:
	/// * `::Json => Err(json_value)`
	/// * `::PlainText => Err(Value::String)`
	/// * `::None => Err(Value::Null)`
	fn as_type(self) -> Result<T, Value> {
		use RawApiResponse::*;
		match self {
			Json(v) => Err(v),
			PlainText(s) => Err(Value::String(s)),
			None => Err(Value::Null),
		}
	}
}*/
impl RawApiResponse {
	pub async fn extract(resp: Response) -> RResult<(u16, RawApiResponse)> {
		let endpoint = resp.url().path().to_string();
		let status = resp.status().as_u16();
		if resp.content_length().expect("CML to respond with a Content-Length header") == 0 {
			Ok((status, RawApiResponse::None))
		} else {
			match resp.headers().get("content-type").map(|s| s.to_owned()) {
				None => Err(ApiError::new(endpoint, ApiErrorType::unknown("response without a Content-Type header")))?,
				Some(ct) => {
					let text = resp.text().await?;
					match ct.to_str().expect("content-type header to contain only ascii") {
						"text/plain; charset=utf-8" => {
							Ok((status, RawApiResponse::PlainText(text)))
						},
						"application/json; charset=utf-8" => {
							serde_json::from_str::<Value>(&text)
								.map_err(|e| ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to parse returned JSON".into(), text, e)).into())
								.map(|as_v| (status, RawApiResponse::Json(as_v)))
							
								/*
							use std::io::BufReader;
							serde_json::from_reader::<_, T>(BufReader::new(text.as_bytes()))
								.map(|t| (status, RawApiResponse::Type(t)))
								.or_else(|_| serde_json::from_str::<Value>(&text)
									.map(|v| (status, RawApiResponse::Json(v)))
								)
								.map_err(|e| ApiError::new(endpoint, ApiErrorType::JsonDecode(text, e)).into())*/
					}
					ct @ _ => Err(ApiError::new(
						endpoint,
						ApiErrorType::unknown(format!("unknown content-type: `{}`", ct)),
					))?,
				}
			}
		}
	}
}
	pub fn as_string(&self) -> Cow<'_, str> {
		match self {
			RawApiResponse::None => Cow::from(""),
			RawApiResponse::PlainText(s) => Cow::from(s),
			RawApiResponse::Json(v) => Cow::from(v.to_string()),
		}
	}
}

fn get_cml_client(token: Option<&str>) -> RResult<Client> {
	// many CML instances are self-signed
	let mut builder = Client::builder().danger_accept_invalid_certs(true);

	if let Some(t) = token {
		let mut hm = HeaderMap::new();
		let val = format!("Bearer {}", t);
		hm.append("Authorization", val.parse().unwrap());

		builder = builder.default_headers(hm);
	}

	builder.build().map_err(|e| CmlError::Network(e))
}
/// Meant for 400 errors
#[derive(Deserialize)]
struct BadRequest {
	pub code: isize,
	pub description: String,
}

#[derive(Debug, Clone)]
pub struct CmlUser {
	client: Client,
	host: String,
	username: String,
	token: String,
	roles: Vec<String>,
}

use crate::rest_types as rt;

#[cfg(feature="untyped_requests")]
pub mod raw {
	use super::CmlUser;

	/// Sets up an authenticated GET request to the CML rest server (version 0).
	#[allow(unused)]
	pub fn get_v0<D: ToString>(user: &CmlUser, endpoint: D) -> reqwest::RequestBuilder {
		user.get_v0(endpoint)
	}

	/// Sets up an authenticated PUT request to the CML rest server (version 0).
	///
	/// Data must be passed into the resulting object before the request is sent.
	#[allow(unused)]
	pub fn put_v0<D: ToString>(user: &CmlUser, endpoint: D) -> reqwest::RequestBuilder {
		user.put_v0(endpoint)
	}
}

// TODO: impl a from_format that special cases "https://{}/{}?{}#{}", etc without allocating

impl CmlUser {
	fn get_v0<E: ToString>(&self, endpoint: E) -> reqwest::RequestBuilder {
		let s = endpoint.to_string();
		let mut as_str: &str = &s;
		if as_str.starts_with('/') { as_str = &as_str[1..]; }
		self.client.get(format!("https://{}/api/v0/{}", self.host, as_str).as_str())
	}
	fn put_v0<E: ToString>(&self, endpoint: E) -> reqwest::RequestBuilder {
		let s = endpoint.to_string();
		let mut as_str: &str = &s;
		if as_str.starts_with('/') { as_str = &as_str[1..]; }
		self.client.put(format!("https://{}/api/v0/{}", self.host, as_str).as_str())
	}

	pub fn host(&self) -> &str {
		&self.host
	}
	pub fn username(&self) -> &str {
		&self.username
	}
	pub fn roles(&self) -> Vec<&str> {
		self.roles.iter().map(|s| s.as_str()).collect()
	}

	/// Get a list of labs visible to the user.
	///
	/// `show_all`: If true (and the user is an admin), returns a list of all labs.
	pub async fn labs(&self, show_all: bool) -> RResult<Vec<String>> {
		Ok(self.get_v0(format_args!("/labs?show_all={}", show_all))
			.send().await?
			.json::<Vec<String>>()
			.await?)
	}

	/// Gets a lab's information. If the lab is not found, returns `Ok(None)`
	pub async fn lab(&self, lab_id: &str) -> RResult<Option<rt::Lab>> {
		let resp = self.get_v0(format_args!("/labs/{}", lab_id))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<rt::Lab>(j.clone()) {
					Ok(t) => Ok(Some(t)),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(404, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<BadRequest>(j.clone()) {
					Ok(t) if t.description.starts_with("Lab not found: ") => Ok(None),
					_ => Err(ApiError::new(endpoint, ApiErrorType::BadResponse("Bad response from server for 404: ".into(), j.to_string())).into()),
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}

	/// Get's a list of the lab's nodes.
	pub async fn lab_nodes(&self, lab_id: &str) -> RResult<Option<Vec<String>>> {
		let resp = self.get_v0(format_args!("/labs/{}/nodes", lab_id))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<Vec<String>>(j.clone()) {
					Ok(t) => Ok(Some(t)),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(404, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<BadRequest>(j.clone()) {
					Ok(t) if t.description.starts_with("Lab not found: ") => Ok(None),
					_ => Err(ApiError::new(endpoint, ApiErrorType::BadResponse("Bad response from server for 404".into(), j.to_string())).into())
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}

	pub async fn lab_node(&self, lab_id: &str, node_id: &str) -> RResult<Option<rt::SimpleNode>> {
		let resp = self.get_v0(format_args!("/labs/{}/nodes/{}?simplified={}", lab_id, node_id, true))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<SimpleNode>(j.clone()) {
					Ok(t) => Ok(Some(t)),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(404, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<BadRequest>(j.clone()) {
					Ok(t) if t.description.starts_with("Lab not found: ") => Ok(None),
					Ok(t) if t.description.starts_with("Node not found: ") => Ok(None),
					_ => Err(ApiError::new(endpoint, ApiErrorType::BadResponse("Bad response from server for 404".into(), j.to_string())).into())
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}

	/// Gets the currently saved configuration for the device. May not match the currently running configuration.
	pub async fn lab_node_config(&self, lab_id: &str, node_id: &str) -> RResult<Option<String>> {
		let resp = self.get_v0(format_args!("/labs/{}/nodes/{}/config", lab_id, node_id))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::PlainText(s)) => {
				Ok(Some(s))
			},
			(404, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<BadRequest>(j.clone()) {
					Ok(t) if t.description.starts_with("Lab not found: ") => Ok(None),
					Ok(t) if t.description.starts_with("Node not found: ") => Ok(None),
					_ => Err(ApiError::new(endpoint, ApiErrorType::BadResponse("Bad response from server for 404".into(), j.to_string())).into())
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}

	pub async fn lab_node_state(&self, lab_id: &str, node_id: &str) -> RResult<Option<rt::State>> {
		#[derive(Serialize, Deserialize)]
		struct StateResponse {
			state: rt::State,
		}

		let resp = self.get_v0(format_args!("/labs/{}/nodes/{}/state", lab_id, node_id))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<StateResponse>(j.clone()) {
					Ok(t) => Ok(Some(t.state)),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(404, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<BadRequest>(j.clone()) {
					Ok(t) if t.description.starts_with("Lab not found: ") => Ok(None),
					Ok(t) if t.description.starts_with("Node not found: ") => Ok(None),
					_ => Err(ApiError::new(endpoint, ApiErrorType::BadResponse("Bad response from server for 404".into(), j.to_string())).into())
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}
	pub async fn lab_node_start(&self, lab_id: &str, node_id: &str) -> RResult<Option<rt::State>> {
		#[derive(Serialize, Deserialize)]
		struct StateResponse {
			state: rt::State,
		}

		let resp = self.put_v0(format_args!("/labs/{}/nodes/{}/state/start", lab_id, node_id))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<Option<StateResponse>>(j.clone()) {
					Ok(t) => Ok(t.map(|s| s.state)),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(404, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<BadRequest>(j.clone()) {
					Ok(t) if t.description.starts_with("Lab not found: ") => Ok(None),
					Ok(t) if t.description.starts_with("Node not found: ") => Ok(None),
					_ => Err(ApiError::new(endpoint, ApiErrorType::BadResponse("Bad response from server for 404".into(), j.to_string())).into())
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}
	pub async fn lab_node_stop(&self, lab_id: &str, node_id: &str) -> RResult<Option<()>> {
		#[derive(Serialize, Deserialize)]
		struct StateResponse {
			state: rt::State,
		}

		let resp = self.put_v0(format_args!("/labs/{}/nodes/{}/state/stop", lab_id, node_id))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<serde_json::Value>(j.clone()) {
					Ok(serde_json::Value::Null) => Ok(Some(())),
					Ok(_) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse("Expected JSON with null value, received something else".into(), j.to_string())).into()),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(404, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<BadRequest>(j.clone()) {
					Ok(t) if t.description.starts_with("Lab not found: ") => Ok(None),
					Ok(t) if t.description.starts_with("Node not found: ") => Ok(None),
					_ => Err(ApiError::new(endpoint, ApiErrorType::BadResponse("Bad response from server for 404".into(), j.to_string())).into())
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}

	pub async fn lab_node_keys_console(&self, lab_id: &str, node_id: &str, line: Option<u64>) -> RResult<Option<String>> {
		let resp = self.get_v0(format_args!("/labs/{}/nodes/{}/keys/console{}", lab_id, node_id, match line {
			Some(l) => format!("?line={}", l),
			None => format!(""),
		}))
		.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				// if it can't find something, it will error - not return null/etc
				match serde_json::from_value::<String>(j.clone()) {
					Ok(t) => Ok(Some(t)),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(400 | 404 | 500, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<BadRequest>(j.clone()) {
					Ok(t) if t.description.starts_with("Lab not found: ") => Ok(None),
					Ok(t) if t.description.starts_with("Serial port does not exist on node: ") => Ok(None),
					// actually node not found
					Ok(t) if t.description.contains("encountered an unexpected error. Please report this problem to support.") => Ok(None),
					_ => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response from server for {}", rresp.0), j.to_string())).into())
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}

	pub async fn lab_topology(&self, lab_id: &str, include_configurations: bool) -> RResult<Option<rt::LabTopology>> {
		let resp = self.get_v0(format_args!("/labs/{}/topology?exclude_configurations={}", lab_id, !include_configurations))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<rt::LabTopology>(j.clone()) {
					Ok(t) => Ok(Some(t)),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(404, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<BadRequest>(j.clone()) {
					Ok(t) if t.description.starts_with("Lab not found: ") => Ok(None),
					_ => Err(ApiError::new(endpoint, ApiErrorType::BadResponse("Bad response from server for 404".into(), j.to_string())).into())
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}

	pub async fn lab_topologies<'b, I: IntoIterator<Item = &'b S>, S: AsRef<str> + 'b>(&'_ self, lab_ids: I, include_configurations: bool) -> RResult<Vec<(&'b str, Option<rt::LabTopology>)>> {
		// TODO: change this to return a HashMap, that skips missing topologies?
		
		async fn get_topo<'a>(client: &'_ CmlUser, s: &'a str, configs: bool) -> RResult<(&'a str, Option<rt::LabTopology>)> {
			let topo = client.lab_topology(s, configs).await?;
			Ok((s, topo))
		}
		
		let futs: Vec<_> = lab_ids.into_iter()
			.map(|id| get_topo(self, id.as_ref(), include_configurations))
			.collect();
		let topos = futures::future::join_all(futs).await
			.into_iter()
			.collect::<RResult<_>>()?;

		Ok(topos)
	}

	/// Returns the currently available console lines. This does not show lines from shutdown devices.
	pub async fn keys_console(&self, show_all: bool) -> RResult<HashMap<String, rt::key::Console>> {
		let resp = self.get_v0(format_args!("/keys/console?show_all={}", show_all))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<HashMap<String, rt::key::Console>>(j.clone()) {
					Ok(t) => Ok(t),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}

	/// Returns the currently available keys for devices capable of VNC. This does not show keys from shutdown or disabled devices.
	pub async fn keys_vnc(&self, show_all: bool) -> RResult<HashMap<String, rt::key::VNC>> {
		let resp = self.get_v0(format_args!("/keys/console?show_all={}", show_all))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<HashMap<String, rt::key::VNC>>(j.clone()) {
					Ok(t) => Ok(t),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}



	/// Extracts the configuration from a running node
	///
	/// Returns None if the lab nor node cannot be found
	pub async fn extract_node_config(&self, lab_id: &str, node_id: &str) -> RResult<Option<String>> {
		let resp = self.put_v0(format_args!("/labs/{}/nodes/{}/extract_configuration", lab_id, node_id))
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<String>(j.clone()) {
					Ok(t) => Ok(Some(t)),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(404, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<BadRequest>(j.clone()) {
					Ok(t) if t.description.starts_with("Lab not found: ") => Ok(None),
					Ok(t) if t.description.starts_with("Node not found: ") => Ok(None),
					_ => Err(ApiError::new(endpoint, ApiErrorType::BadResponse("Bad response from server for 404".into(), j.to_string())).into())
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}

	pub async fn simplified_node_definitions(&self) -> RResult<Vec<rt::SimpleNodeDefinition>> {
		let resp = self.get_v0("/simplified_node_definitions")
			.send().await?;
		let endpoint = resp.url().path().to_owned();
		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<Vec<rt::SimpleNodeDefinition>>(j.clone()) {
					Ok(t) => Ok(t),
					Err(sje) => Err(ApiError::new(endpoint, ApiErrorType::JsonDecode("Unable to read JSON response as a proper type".into(), j.to_string(), sje)).into()),
				}
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::BadResponse(format!("Bad response for status {}", status), format!("{:?}", resp))).into()),
		}
	}
}

#[derive(Debug)]
pub struct Authenticate {
	pub host: String,
	pub username: String,
	pub password: String,
}
impl Authenticate {
	pub async fn login(&self) -> RResult<CmlUser> {
		#[derive(Debug, Deserialize)]
		struct RespAuthExtended {
			username: String,
			token: String,
			roles: Vec<String>,
		}
		#[derive(Serialize)]
		struct ReqAuth<'a> {
			username: &'a str,
			password: &'a str,
		}

		let client = get_cml_client(None)?;
		let endpoint = format!("https://{host}/api/v0/auth_extended", host = self.host);
		let resp = client.post(&endpoint)
			.json(&ReqAuth { username: &self.username, password: &self.password })
			.send().await?;
		
		let endpoint = resp.url().path().to_owned();
		//let status = resp.status().as_u16();

		let rresp = RawApiResponse::extract(resp).await?;

		match rresp {
			(200, RawApiResponse::Json(j)) => {
				match serde_json::from_value::<RespAuthExtended>(j.clone()) {
					Ok(rae) => Ok(CmlUser {
						client: get_cml_client(Some(&rae.token))?,
						host: self.host.clone(),
						username: rae.username,
						token: rae.token,
						roles: rae.roles,
					}),
					Err(_) => {
						Err(ApiError::new(endpoint, ApiErrorType::BadResponse("received json did not match expected json for successful response".into(), j.to_string())).into())
					}
				}
			},
			(403, RawApiResponse::Json(Value::String(s))) if s == "Authentication failed!" => {
				Err(ApiError::new(endpoint, ApiErrorType::AuthenticationFailure).into())
			},
			(status @ _, resp @ _) => Err(ApiError::new(endpoint, ApiErrorType::unknown(format!("Unknown response from server (status code = {}): {:?}", status, resp))).into()),
		}
	}
}
