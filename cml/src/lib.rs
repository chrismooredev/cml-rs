

//extern crate serde;

pub mod lab_yaml;
pub mod rest_types;
pub mod rest;

pub use rest_types as rt;

pub const ENV_CML_HOST: &str = "CML_HOST";
pub const ENV_CML_USER: &str = "CML_USER";
pub const ENV_CML_PASS: &str = "CML_PASS";
pub const ENV_CML_PASS64: &str = "CML_PASS64";

/// Used to get Authentication info from environment variables (CML_HOST, CML_USER, CML_PASS64, CML_PASS)
pub fn get_auth_env() -> Result<rest::Authenticate, String> {
	// only support via ENV args for now
	let host = std::env::var(ENV_CML_HOST);
	let user = std::env::var(ENV_CML_USER);
	let pass = std::env::var(ENV_CML_PASS);
	let pass64 = std::env::var(ENV_CML_PASS64);

	let host = host
		.map_err(|_| format!("Missing or invalid environment variable `{}`", ENV_CML_HOST))?;
	let user = user
		.map_err(|_| format!("Missing or invalid environment variable `{}`", ENV_CML_USER))?;
	let pass: String = pass64.ok()
		.and_then(|s| base64::decode(s.as_str()).ok())
		.and_then(|vu8| String::from_utf8(vu8).ok())
		.or(pass.ok())
		.ok_or_else(|| format!("Missing or invalid environment variable(s): `{}` or `{}`", ENV_CML_PASS64, ENV_CML_PASS))?;

	Ok(rest::Authenticate {
			host,
			username: user,
			password: pass,
	})
}
