#![feature(or_patterns)]

use ascii::{AsciiChar};
use termion::event::{Event, Key};
use smol_str::SmolStr;

pub mod listing;

/**
Maps keyboard key events to a character sequence to send to a terminal.

Returns a nested result - The outer result signifies a if there is a matching code, the inner result contains the actual key codes, depending on if it can be a char or constant string.
*/
pub fn event_to_code(event: Event) -> Result<SmolStr, String> {
	//use AsciiChar::*;

	macro_rules! esc {
		($s: literal) => {
			// AsciiChar::ESC + $s
			SmolStr::new_inline(concat!('\x1B', $s))
		}
	}

	const ARROW_UP: SmolStr = esc!("[A");
	const ARROW_DOWN: SmolStr = esc!("[B");
	const ARROW_RIGHT: SmolStr = esc!("[C");
	const ARROW_LEFT: SmolStr = esc!("[D");

	/*
	special handling:
		ctrl-arrow keys are mapped to 
		ctrl+shift+6 x -> alt+6
	*/
	

	let code: Result<char, SmolStr> = match event {
		Event::Key(key) => match key {
			Key::Char(ch) => Ok(ch),

			// make these "control-codes" emit as arrow keys instead
			Key::Ctrl(ch @ ('p' | 'n' | 'f' | 'b')) => Err((match ch {
				'p' => ARROW_UP,
				'n' => ARROW_DOWN,
				'f' => ARROW_RIGHT,  // right - responds with <char at new position>
				'b' => ARROW_LEFT,  // left - responds with <bksp>
				_ => unreachable!(),
			}).into()),
			//Key::Ctrl('m') => Ok('\n'),
			Key::Ctrl(ch) => match AsciiChar::from_ascii(ch) {
				Err(e) => { Err(format!("Attempt to use non-ascii control character: {} ({:?})", ch, e))? },
				Ok(ac) => match ascii::caret_decode(ac.to_ascii_uppercase()) {
					None => { Err(format!("No control-character for ascii character '{}'", ac))? },
					Some(ctrl_ac) => Ok(ctrl_ac.as_char()),
					/*
						references:
							https://etherealmind.com/cisco-ios-cli-shortcuts/

					*/
					/* IOS functions for various keys:
						^A -> Move cursor to beginning of line
						^B -> Move cursor backwards one character
						ESC B -> Move backwards one word
						^C -> Exit, Exit from config mode
						ESC C -> Make letter uppercase
						^D -> Delete current character (mapped to DEL)
						^D -> EOF, Captured by shell to close console connection
						ESC D -> Remove char to right of cursor
						^E -> Move cursor to end of line
						^F -> Move cursor forward one character
						ESC F -> Move cursor forward one word
						^G
						^H
						^I
						^J
						^K -> Delete line from cursor to end
						^L -> Reprint line
						ESC L -> Make letter lowercase
						^M
						^N, Down -> Next Command
						^O
						^P, Up -> Previous Command
						^Q
						^R -> Refresh Line (Start new line, with same command)
						^S
						^T -> Swap current and previous characters
						^U -> Delete whole line
						ESC U -> Make rest of word uppercase
						^V
						^W -> Delete word to left of cursor
						^X -> Delete line from cursor to start (Stores deleted text in deleted buffer)
						^Y -> Paste most recent entry in delete buffer
						ESC Y -> Paste previous entry in history buffer
						^Z -> Apply command, Exit from config mode

						Ctrl+Shift+6, x -> Break current command (Mapped to ALT+6)
					*/
				}
			},
			/*Key::Ctrl(ch) => Ok(match ch {
				// replace with ascii::caret_decode ?

				'a' => AsciiChar::SOH, // Start of Heading (move cursor to beginning of line)
				'b' => AsciiChar::SOX, // STX - Start of Text (move cursor backward)
				'c' => AsciiChar::ETX, // End of Text
				'd' => AsciiChar::EOT, // End of Transmission
				'e' => AsciiChar::ENQ, // ENQ - Enquiry (move cursor to end of line)
				'k' => AsciiChar::VT, //"\x0B",  // (Delete line from cursor to end)
				'l' => AsciiChar::FF, // "\x0C",  // FF - Form Feed (Reprint the line)
				'u' => AsciiChar::NAK, // "\x15",  // NAK - Neg Ack (Delete line from cursor to beginning)
				'z' => AsciiChar::SUB, // "\x1A",  // SUB - Substitute

				// https://www.cisco.com/c/en/us/td/docs/wireless/controller/7-0/command/reference/cli70bk/cli70over.pdf
				
				'm' => AsciiChar::LineFeed, //"\n",  // (Enter an 'Enter' or 'Return' character from anywhere in the line)
				't' => AsciiChar::Tab, //"\t",  // (Expand the cmd/abbreviation)
				// 'w' => ???, // (Delete word left of cursor) (cannot get sequence from browser)


				// https://etherealmind.com/cisco-ios-cli-shortcuts/
				//'p' => "\x10", // DLE - Data Link Escape (previous command)

				//'n' => // Next command
				_ => todo!("ahh"),
			}.as_char()),*/
			
			Key::Up => Err(ARROW_UP.into()),
			Key::Down => Err(ARROW_DOWN.into()),
			Key::Right => Err(ARROW_RIGHT.into()),
			Key::Left => Err(ARROW_LEFT.into()),
			Key::Home => Ok(AsciiChar::SOH.as_char()),
			Key::End => Ok(AsciiChar::ENQ.as_char()),
			Key::Delete => Ok(AsciiChar::EOT.as_char()), // remove char to right of cursor (ctrl+d ?)
			Key::Esc => Ok(AsciiChar::ESC.as_char()), // ESC - Escape
			Key::Backspace => Ok(AsciiChar::BackSpace.as_char()),

			Key::Alt('6') => {
				//ctrl+shift+6, x
				// [AsciiChar::RS, 'x']
				Err(SmolStr::new("\x1Ex"))
			},

			// experimental section

			c @ _ => Err(format!("unexpected key: '{:?}'", c))?,
			// PageUp, PageDown, BackTab, Delete, Insert, F(u8), Alt(char), Null, Esc
		},
		Event::Mouse(mouse_event) => {
			Err(format!("unknown mouse input event: {:?}", mouse_event))?
		},
		Event::Unsupported(v) if matches!(v.as_slice(), b"\x1B[1;5D") => {
			/* Ctrl+Left Arrow */
			Err(esc!('b'))
		},
		Event::Unsupported(v) if matches!(v.as_slice(), b"\x1B[1;6D") => {
			/* Ctrl+Right Arrow */
			Err(esc!('f'))
		},
		Event::Unsupported(v) => {
			eprintln!("unknown input event, sending as-is (on next tx line)");
			let s = String::from_utf8_lossy(&v);
			assert_eq!(s.as_bytes(), &v);
			Err(SmolStr::new(s))
		},
	};

	match code {
		Ok(c) => {
			let mut buf = [0u8; 4];
			let s = c.encode_utf8(&mut buf);
			Ok(SmolStr::new(s))
		},
		Err(ss) => Ok(ss),
	}
}

