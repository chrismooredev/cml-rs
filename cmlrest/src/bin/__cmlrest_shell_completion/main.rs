#![feature(result_flattening)]

use std::convert::TryFrom;
use std::env::VarError;
use std::ffi::OsString;
use std::io;

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
	exe: Option<String>,
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
	
			Some(words.into_iter()
				.map(|vb| String::from_utf8(vb).expect("command line not UTF8"))
				.collect())
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
			let mut exe: Option<String> = None;
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
                    exe = Some(args[ep + 1].clone());
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
}

#[tokio::main]
async fn main() -> io::Result<()> {
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
		},
	};

	// only use bash-specific stuff for now
	let cword = ctx.word.unwrap();
	let pword = ctx.prev_word.unwrap();

	let completes: Vec<String> = if pword == "lab" {
		let auth_info = cml::get_auth_env().expect("[completions] error getting auth info");
		let client = auth_info.login().await.expect("[completions] error logging into CML");
		let labs = client.labs(true).await.expect("[completions] error getting labs from CML");

		labs.into_iter()
			.filter(|l| (cword.len() == 0) || l.starts_with(&cword))
			.collect()
	} else {
		Vec::new()
	};

	for c in completes {
		print!("{}\0", c);
	}
	//print!("{}", completes.join("\0"));

	Ok(())
}
