
use std::collections::VecDeque;
use thiserror::Error;

macro_rules! esc {
	($s: literal) => {
		// AsciiChar::ESC + $s
		SmolStr::new_inline(concat!('\x1B', $s))
	};
}

//pub mod expose;
pub mod listing;
pub mod api;
pub mod terminal;
pub mod term;
pub mod open_term;

#[derive(Debug, Error)]
pub enum TerminalError {
	#[error("An error occured invoking the CML Rest API")]
	Cml(#[from] cml::rest::Error),
	#[error("Bad command line usage")]
	BadUsage,
	#[error("Unable to find the node within the lab")]
	BadDeviceQuery(#[from] terminal::NodeSearchError),
	#[error("Line not found on resolved node")]
	BadLineQuery(#[from] terminal::ConsoleSearchError),
	#[error("Completions script not available for {}", .0)]
	UnsupportedCompletionShell(String),

	#[error(transparent)]
	DriverError(#[from] anyhow::Error),
}

// misc private utilities

/// Adds data to the rotating buffer, and makes it contiguous.
/// Only fills the buffer to the buffer's capacity, and rotates data in terms of that.
/// In other words, this function will not allocate.
fn update_chunkbuf(buffer: &mut VecDeque<u8>, chunk: &[u8]) {
	// update our cache, later notify listeners

	// shorten the VecDeque to a length capable of holding the data without exceeding capacity
	// then fill the VecDeque with our new data chunk

	if buffer.len() + chunk.len() <= buffer.capacity() {
		// we can hold this chunk without removing elements
		buffer.extend(chunk);
	} else if chunk.len() > buffer.capacity() {
		// this will extend capacity - we want to be able to hold at least the size of each chunk
		buffer.clear();
		buffer.extend(chunk);
	} else {
		// we must delete some elements to hold this chunk
		let start_ind = buffer.capacity() - chunk.len();
		let rotate_amt = buffer.len() .min( chunk.len() );
		
		buffer.rotate_left(rotate_amt);
		buffer.resize(start_ind, 0);
		buffer.extend(chunk);
	}

	buffer.make_contiguous();
}

fn find_slice<T>(haystack: &[T], needle: &[T]) -> Option<usize>
	where for<'a> &'a [T]: PartialEq
{
	// https://stackoverflow.com/a/35907071/11536614
	haystack.windows(needle.len()).position(|window| window == needle)
}

