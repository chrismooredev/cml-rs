
mod tty;
mod script;

pub mod types {
	pub use super::tty::UserTerminal;
	pub use super::script::ScriptedTerminal;
}
