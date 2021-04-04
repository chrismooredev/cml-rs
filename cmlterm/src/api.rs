use std::str::FromStr;



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptWaitCondition<S> {
	Prompt,
	None,
	Static(S),
}
impl<S> ScriptWaitCondition<S> {
	pub fn map<T, F: FnOnce(S) -> T>(self, f: F) -> ScriptWaitCondition<T> {
		use ScriptWaitCondition::*;
		match self {
			None => None,
			Prompt => Prompt,
			Static(s) => Static(f(s)),
		}
	}
	pub fn into<T: From<S>>(self) -> ScriptWaitCondition<T> {
		self.map(<T as From<S>>::from)
	}
}
impl<'a> ScriptWaitCondition<&'a str> {
	pub fn from_line(line: &'a str) -> (usize, ScriptWaitCondition<&'a str>) {
		if line.starts_with("\\~") || line.starts_with("\\`") {
			(1, ScriptWaitCondition::Prompt)
		} else if line.starts_with('~') {
			(1, ScriptWaitCondition::None)
		} else if line.starts_with('`') {
			line.char_indices()
				.filter(|(i, _)| *i != 0) // not the first one
				.filter(|(_, c)| *c == '`') // we are a grave
				.filter(|(i, _)| ! line[..*i].ends_with('\\')) // previous is not a backslash
				.map(|(i, _)| &line[1..i]) // string between the two graves
				.next()
				.map(|l| (2+l.len(), ScriptWaitCondition::Static(l)))
				.unwrap_or((0, ScriptWaitCondition::Prompt))
		} else {
			(0, ScriptWaitCondition::Prompt)
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitMode {
	Prompt,
	Immediate,
	Expect(usize, String),
}
impl Default for WaitMode { fn default() -> WaitMode { WaitMode::Prompt }}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptCommand {
	pub wait_mode: WaitMode,
	pub skip_newline: bool,
	pub timeout: Option<u64>,
	pub command: String,
}
impl ScriptCommand {
	pub fn basic<S: Into<String>>(s: S) -> ScriptCommand {
		ScriptCommand {
			command: s.into(),
			..Default::default()
		}
	}

	pub fn immediate(&self) -> bool {
		self.wait_mode == WaitMode::Immediate
	}
	pub fn timeout(&self) -> Option<u64> {
		self.timeout
	}
	pub fn skip_newline(&self) -> bool {
		self.skip_newline
	}
}
impl FromStr for ScriptCommand {
	type Err = String;
	fn from_str(s: &str) -> Result<ScriptCommand, Self::Err> {
		let mut rest = s;
		//let mut modifiers = Vec::new();
		//let mut wait_mode = WaitMode::Prompt;
		let mut scmd = ScriptCommand::default();
		loop {
			if rest.starts_with('~') {
				rest = &rest[1..];
				scmd.wait_mode = WaitMode::Immediate;
			} else if rest.starts_with('!') {
				rest = &rest[1..];
				scmd.skip_newline = true;
			} else if rest.starts_with('`') {
				let ending = rest.char_indices()
					.skip(1)
					.find(|&(_, c)| c == '`');
				match ending {
					None => return Err(s.into()),
					Some((i, _)) => {
						let inner = &rest[1..i];
						match inner.char_indices().find(|(_, c)| *c == ':') {
							None => return Err(s.into()),
							Some((i, _)) => {
								let (timeout, expect) = (&inner[..i], &inner[i+1..]);
								let num = match timeout.parse() {
									Err(_) => return Err(s.into()),
									Ok(n) => n,
								};
								scmd.wait_mode = WaitMode::Expect(num, expect.into());
							}
						}
						
						rest = &rest[i+1..];
						
					}
				}
			} else if rest.starts_with('%') {
				let ending = rest.char_indices()
					.skip(1)
					.find(|(_, c)| *c == '%');
				match ending {
					None => return Err(s.into()),
					Some((i, _)) => {
						let inner = &rest[1..i];
						
						let num: u64 = match inner.parse() {
							Err(_) => return Err(s.into()),
							Ok(n) => n,
						};
						scmd.timeout = Some(num);

						rest = &rest[i+1..];
					}
				}
			} else if rest.starts_with('\\') && rest.starts_with(|c| matches!(c, '!' | '~')) {
				// skip past the escape
				rest = &rest[1..];
			} else {
				break;
			}
		}
		scmd.command = rest.into();
		Ok(scmd)
	}
}

