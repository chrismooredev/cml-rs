#![feature(result_flattening)]
#![feature(try_blocks)]
#![feature(bool_to_option)]
#![feature(drain_filter)]

use std::convert::TryFrom;
use std::env::VarError;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use tokio::runtime::Runtime;

use cml::rest::CmlUser;
type CmlResult<T> = Result<T, cml::rest::Error>;

#[derive(Debug, Clone)]
enum CompletionError {
	NotUnicode(OsString),
	MissingEnvVar,
	/// Unable to parse an env var into the proper type. BadEnvVarType(expected_type, found_string)
	BadEnvVarType(&'static str, String),
	BadCompletionKey(String),
}
impl CompletionError {
	fn env_var_optional<'a, T, F: FnOnce(String) -> Result<T, CompletionError>>(var_name: &'a str, map: F) -> Result<Option<T>, (&'a str, CompletionError)> {
		let result = match std::env::var(var_name) {
			Err(VarError::NotPresent) => Ok(None),
			Err(VarError::NotUnicode(osstr)) => Err(CompletionError::NotUnicode(osstr)),
			Ok(s) => Ok(Some(s)),
		};

		result
			.map(|o| o.map(map).transpose())
			.flatten()
			.map_err(|ce| (var_name, ce))
	}
	fn env_var_required<'a, T, F: FnOnce(String) -> Result<T, CompletionError>>(var_name: &'a str, map: F) -> Result<T, (&'a str, CompletionError)> {
		CompletionError::env_var_optional(var_name, map)
			.and_then(|o| match o {
				Some(v) => Ok(v),
				None => Err((var_name, CompletionError::MissingEnvVar))
			})
	}
}
impl From<VarError> for CompletionError {
	fn from(ve: VarError) -> CompletionError {
		match ve {
			VarError::NotPresent => CompletionError::MissingEnvVar,
			VarError::NotUnicode(osstr) => CompletionError::NotUnicode(osstr),
		}
	}
}

#[derive(Debug, Clone, Copy)]
enum CompletionKey {
	Normal,                  // '\t' 9
	SuccessiveTabs,          // '?' 63
	PartialWordAlternatives, // '!' 33
	Unmodified,              // '@' 64
	Menu,                    // '%' 37
}
impl TryFrom<char> for CompletionKey {
	type Error = CompletionError;
	fn try_from(c: char) -> Result<CompletionKey, Self::Error> {
		Ok(match c {
			'\t' => CompletionKey::Normal,
			'?' => CompletionKey::SuccessiveTabs,
			'!' => CompletionKey::PartialWordAlternatives,
			'@' => CompletionKey::Unmodified,
			'%' => CompletionKey::Menu,
			_ => return Err(CompletionError::BadCompletionKey(c.to_string())),
		})
	}
}
impl TryFrom<&str> for CompletionKey {
	type Error = CompletionError;
	fn try_from(s: &str) -> Result<Self, Self::Error> {
		let as_u8 = s.parse::<u8>()
			.map_err(|_| CompletionError::BadEnvVarType("u8", s.into()))?;
		let c = char::from(as_u8);
		CompletionKey::try_from(c)
	}
}

#[derive(Debug, Clone)]
struct CompletionVars {
	/// The current command line
	line: String, // COMP_LINE
	/// The index of the cursor into `self.line`
	cursor: usize, // COMP_POINT

	/// The key (or final key of a key sequence) used to invoke the current completion function
	key: Option<CompletionKey>, // COMP_KEY

	reqtype: Option<CompletionKey>, // COMP_TYPE

	/// Set of characters that are considered word seperators
	wordbreaks: Option<Vec<char>>, // COMP_WORDBREAKS

	/// The line split up into discrete words
	words: Option<Vec<String>>,
	/// The executable name for the completion
	exe: Option<PathBuf>,
	/// The current word to complete
	word: Option<String>,
	/// The word before the current word to complete
	prev_word: Option<String>,
}

impl CompletionVars {
	fn from_bash_env(use_stdin: bool, use_args: bool) -> Result<CompletionVars, (&'static str, CompletionError)> {
		let line: String = CompletionError::env_var_required("COMP_LINE", |s| Ok(s))?;
		let cursor: usize = CompletionError::env_var_required("COMP_POINT", |s| {
			s.parse().map_err(|_| CompletionError::BadEnvVarType("64-bit decimal integer", s))
		})?;

		let key: Option<CompletionKey> = CompletionError::env_var_optional("COMP_KEY", |s| CompletionKey::try_from(s.as_str()))?;
		let reqtype: Option<CompletionKey> = CompletionError::env_var_optional("COMP_TYPE", |s| CompletionKey::try_from(s.as_str()))?;

		let words: Option<Vec<String>> = if use_stdin {
			use io::Read;
			let stdin = io::stdin();
			let stdlock = stdin.lock();
			let mut words: Vec<Vec<u8>> = Vec::with_capacity(8);
			let mut tmp_vec: Vec<u8> = Vec::with_capacity(64);
			for byte in stdlock.bytes() {
				let b = byte.expect("IO Error when reading stdin");
				// each word is to be null-terminated, not null-seperated
				if b == '\0' as u8 {
					words.push(tmp_vec);
					tmp_vec = Vec::with_capacity(64);
				} else {
					tmp_vec.push(b);
				}
			}

			Some(
				words
					.into_iter()
					.map(|vb| String::from_utf8(vb).expect("command line not UTF8"))
					.collect(),
			)
		} else {
			None
		};

		let (wordbreaks, exe, word, prev_word) = if use_args {
			/*#[derive(Clap)]
						struct Args {
							/// A list of characters used to seperate words
							#[clap(long)]
							wordbreaks: Option<String>,

							/// The executable name for the completion
							#[clap(long)]
							exe: Option<String>,

							/// The current word to complete
							#[clap(long)]
							word: Option<String>,

							/// The word before the current word to complete
							#[clap(long)]
							prev_word: Option<String>,
						}

						let args = Args::parse();
			*/

			let mut wordbreaks: Option<Vec<char>> = None;
			let mut exe: Option<PathBuf> = None;
			let mut word: Option<String> = None;
			let mut prev_word: Option<String> = None;

			let args: Vec<String> = std::env::args().collect();
			let mut found_args = 0;
			if let Some(wbp) = args.iter().position(|s| s == "--wordbreaks") {
				if wbp + 1 < args.len() {
					wordbreaks = Some(args[wbp + 1].chars().collect());
					found_args += 1;
				}
			}
			if let Some(ep) = args.iter().position(|s| s == "--exe") {
				if ep + 1 < args.len() {
					exe = Some(Path::new(&args[ep + 1]).to_path_buf());
					found_args += 1;
				}
			}
			if let Some(wp) = args.iter().position(|s| s == "--word") {
				if wp + 1 < args.len() {
					word = Some(args[wp + 1].clone());
					found_args += 1;
				}
			}
			if let Some(pwp) = args.iter().position(|s| s == "--prev-word") {
				if pwp + 1 < args.len() {
					prev_word = Some(args[pwp + 1].clone());
					found_args += 1;
				}
			}

			if args.len() != 1 + found_args * 2 {
				eprintln!("shell completer got unexpected arguments.");
				eprintln!("Usage: <shell completer binary> [OPTIONS...]");
				eprintln!("\t--wordbreaks <CHAR LIST> \tA single string containing characters the shell uses to seperate words");
				eprintln!("\t--exe <STRING>           \tThe user-provided string describing the exe to that is to be completed for");
				eprintln!("\t--word <STRING>          \tThe word the user is currently on");
				eprintln!("\t--prev-word <STRING>     \tThe word before the word the user is currently on");
				eprintln!("");
				eprintln!("note: got arguments: {:?}", args);
			}

			(wordbreaks, exe, word, prev_word)
		} else {
			(None, None, None, None)
		};

		Ok(CompletionVars {
			line,
			cursor,
			key,
			reqtype,

			words,

			wordbreaks,
			exe,
			word,
			prev_word,
		})
	}

	fn has_flag(&self, s: char, l: &str) -> bool {
		// self.line.ends_with(" $flag") || self.line.contains(" $flag ")
		
		let mut buf = String::with_capacity(l.len() + 2);
		buf.push(' ');
		buf.push_str(l);
		if self.line.ends_with(&buf) { return true; }
		buf.push(' ');
		if self.line.contains(&buf) { return true; }

		buf.clear();
		buf.push_str(" -");
		buf.push(s);
		if self.line.ends_with(&buf) { return true; }
		buf.push(' ');
		if self.line.contains(&buf) { return true; }

		false
	}

	fn suggest_flag<'a, II: IntoIterator<Item = &'a (&'a str, &'a str)>>(&self, only_on_dash: bool, s: II) -> Vec<&'a str> {
		eprintln!("suggesting flags...");
		//let words: Vec<_> = self.line.split(' ').collect();
		match &self.word {
			Some(cword) if !only_on_dash || cword.starts_with('-') => {
				s.into_iter()
					// eliminate already existing flags
					//.filter(|(s, l)| !(words.contains(s) || words.contains(l)))
					.filter(|(s, l)| !self.has_flag(s.chars().nth(1).unwrap(), l))
					.inspect(|desc| eprintln!("flag does not exist: {:?}", desc))
					.filter(|(_, l)| {
						!cword.starts_with("--") || (cword.starts_with("--") && (cword.len() == 2 || l.starts_with(cword)))
					})
					.inspect(|desc| eprintln!("flag is compatible: {:?}", desc))
					.map(|(_, l)| *l)
					.collect()
			}
			Some(_) | None => Vec::new(),
		}
	}
}
enum QuoteStyle {
	None,
	Single,
	Double,
}
fn bash_escape(s: String, style: QuoteStyle) -> String {
	match style {
		QuoteStyle::None => {}
		QuoteStyle::Single => todo!("single quote arguments"),
		QuoteStyle::Double => todo!("double quote arguments"),
	};

	let mut rtn = String::new();
	for c in s.chars() {
		if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':') {
			rtn.push(c);
		} else {
			rtn.push('\\');
			rtn.push(c);
		}
	}

	rtn
}

async fn get_nodes(client: &CmlUser, all: bool) -> CmlResult<Vec<(String, String, Vec<(String, String)>)>> {
	async fn lab_data(client: &CmlUser, id: String, all: bool) -> CmlResult<Option<(String, String, Vec<(String, String)>)>> {
		match client.lab_topology(&id, false).await? {
			None => Ok(None),
			Some(t) => {
				if all || (client.username() == t.owner && t.state.active()) {
					let title = t.title;
					let nodes: Vec<_> = t.nodes.into_iter()
						.filter(|n| all || n.data.state.active())
						.map(|n| (n.id, n.data.label))
						.collect();
					Ok(Some((id, title, nodes)))
				} else {
					Ok(None)
				}
			}
		}
	}
	
	let lab_list: Vec<String> = client.labs(all).await?;

	let lab_topos_futs: Vec<_> = lab_list.into_iter()
		.map(|lid| lab_data(client, lid, all))
		.collect();
	let lab_topos = futures::future::join_all(lab_topos_futs).await
		.into_iter()
		.filter_map(|k| k.transpose())
		.collect::<CmlResult<Vec<_>>>();
		
	lab_topos
}

fn remove_matching<S: AsRef<str>, F: Fn(&str) -> bool>(v: &mut Vec<S>, f: F) {
	let f = &f;
	while let Some(i) = v.iter().map(|s| s.as_ref()).position(f) {
		v.remove(i);
	}
}

async fn perform_completions(ctx: CompletionVars) -> CmlResult<Vec<String>> {
	let ctx = &ctx;
	// only use bash-specific stuff for now
	let cword = ctx.word.as_ref().unwrap();
	let pword = ctx.prev_word.as_ref().unwrap();
	let words = ctx.words.as_ref().unwrap();
	let words = words.iter().map(|s| s.as_str()).collect::<Vec<_>>();

	let completes: Vec<String> = if pword.ends_with("cmlterm") {
		["list", "open", "expose", /* run */]
			.iter()
			.filter(|so| cword.len() == 0 || so.starts_with(cword))
			.map(|s| s.to_string() + " ")
			.collect()
	} else if let Some(&"list") = words.get(1) {
		// --vnc and --links are incompatible
		let is_vnc = words.contains(&"-v") || words.contains(&"--vnc");
		let is_links = words.contains(&"-l") || words.contains(&"--links");
		eprintln!("(is_vnc, is_links) = {:?}", (is_vnc, is_links));

		let mut c = ctx.suggest_flag(false,
			&[
				("-a", "--all"),
				("-j", "--json"),
				("-v", "--vnc"),
				("-l", "--links"),
			],
		);
		eprintln!("suggested flags: {:?}", c);

		// remove VNC/JSON suggestions if the other exists
		if is_vnc {
			remove_matching(&mut c, |s| s == "--links");
		}
		if is_links {
			remove_matching(&mut c, |s| s == "--vnc");
		}

		// add spaces to be ready for the next flag/arg
		let c: Vec<String> = c.into_iter().map(|s| s.to_string() + " ").collect();

		c
	} else if let Some(&"open") = words.get(1) {
		/*
		if ! words.contains(&"--") && cword.starts_with('-') {
			let opts = ["--vnc"];
		}
		*/

		let mut completes: Vec<String> = Vec::new();
		let show_all = ctx.has_flag('b', "--boot");

		let auth = cml::get_auth_env().unwrap();
		let client = auth.login().await?;

		if cword.starts_with('-') {
			let flags = ctx.suggest_flag(true,
				&[
					("-b", "--boot"),
				],
			);
			flags.iter().for_each(|s| completes.push(s.to_string()));
		}

		let curr_keys = client.keys_console(show_all).await?;
		if cword.len() == 0 || cword.starts_with('/') {
			// /lab_<id/name>/[node_<id/name>/[line = 0]]
		
			// complete for lab IDs/names
			let nodes = get_nodes(&client, show_all).await?;

			match cword.get(1..).map(|s| s.find('/')).flatten() {
				None => {
					nodes.into_iter()
						.fold(Vec::new(), |mut v, (id, name, _)| {
							v.push(id); v.push(name); v
						})
						.into_iter()
						.map(|mut s| {
							s.insert(0, '/');
							s.push('/');
							bash_escape(s, QuoteStyle::None)
						})
						.for_each(|s| completes.push(s));
				},
				Some(i_m1) => {
					let i = i_m1+1;
					// validate lab ID/name
					// complete for node IDs/names
					// line?

					let lab_desc = &cword[1..i];
					eprintln!("lab descriptor: {:?}", lab_desc);

					let lab_info = nodes.into_iter()
						.find(|(id, name, _)| lab_desc == id || lab_desc == name)
						.map(|(_, _, nodes)| nodes);

					if let Some(nodes) = lab_info {
						// validated lab - find node IDs/names

						// use the same term the user used, don't try to change it
						let front = format!("/{}/", lab_desc);

						nodes.into_iter()
							.for_each(|(mut nid, mut nname)| {
								nid.insert_str(0, &front);
								nname.insert_str(0, &front);
								completes.push(bash_escape(nid, QuoteStyle::None));
								completes.push(bash_escape(nname, QuoteStyle::None));

								// user may add trailing slash if they want to specify a line, otherwise default to line=0
							});
					} else {
						// found no lab by that name - do no autocompletion
					}
				}
			}
		}
		if cword.len() == 0 || cword.starts_with(char::is_alphanumeric) {
			// UUID

			// cannot show unbooted nodes
			// it would be bad to boot each and every node just for completion's sake
			curr_keys
				.into_iter()
				// add trailing space to "complete" the UUID argument
				.for_each(|(k, _)| completes.push(k + " "));
		}

		completes.drain_filter(|s| cword.len() != 0 && !s.starts_with(cword)).count();
		
		completes
	} else {
		todo!("unimplemented shell completion case");
	};

	// must perform the escaping at generation, cannot blindly escape all
	/*let completes: Vec<_> = completes.into_iter()
	.map(|s| bash_escape(s, QuoteStyle::None))
	.collect();*/

	Ok(completes)
}

async fn doit(ctx: CompletionVars) -> Result<Vec<String>, Box<dyn std::error::Error>> {
	let r = perform_completions(ctx).await;
	r.map_err::<Box<dyn std::error::Error>, _>(|e| Box::new(e) as Box<dyn std::error::Error>)
}

//#[tokio::main]
fn main() -> io::Result<()> {
	let test_env = std::env::var_os("TEST_ENV").is_some();

	if test_env {
		eprintln!("");
		eprintln!("{:?}", std::env::args().collect::<Vec<_>>());
		eprintln!("");
		std::env::vars()
			.filter(|(n, _)| n.starts_with("COMP"))
			.for_each(|(n, v)| {
				eprintln!("{}: `{}`", n, v);
			});
		eprintln!("");
	}

	let as_struct = CompletionVars::from_bash_env(true, true);
	if test_env {
		eprintln!("data: {:#?}", as_struct);
		eprintln!("");
	}

	let ctx = match as_struct {
		Ok(c) => c,
		Err(e) => {
			eprintln!("error getting completion context: {:?}", e);
			return Ok(());
		}
	};

	let rt = Runtime::new().unwrap();
	let completes = rt.block_on(doit(ctx)).unwrap();

	if test_env {
		eprintln!("completes: {:?}", completes);
		eprintln!("");
	}

	for c in completes {
		print!("{}\0", c);
	}
	//print!("{}", completes.join("\0"));

	Ok(())
}
