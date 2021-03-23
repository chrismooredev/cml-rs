

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptWaitCondition<S> {
	Prompt,
	None,
	StaticString(S),
}
impl<S> ScriptWaitCondition<S> {
	pub fn map<T, F: FnOnce(S) -> T>(self, f: F) -> ScriptWaitCondition<T> {
		use ScriptWaitCondition::*;
		match self {
			None => None,
			Prompt => Prompt,
			StaticString(s) => StaticString(f(s)),
		}
	}
	pub fn into<T: From<S>>(self) -> ScriptWaitCondition<T> {
		self.map(<T as From<S>>::from)
	}
	pub fn to_owned(&self) -> ScriptWaitCondition<S::Owned> where S: ToOwned + Copy {
		(*self).map(|s| s.to_owned())
	}
	pub fn into_owned(self) -> ScriptWaitCondition<S::Owned> where S: ToOwned {
		self.map(|s| s.to_owned())
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
				.map(|l| (2+l.len(), ScriptWaitCondition::StaticString(l)))
				.unwrap_or((0, ScriptWaitCondition::Prompt))
		} else {
			(0, ScriptWaitCondition::Prompt)
		}
	}
}

