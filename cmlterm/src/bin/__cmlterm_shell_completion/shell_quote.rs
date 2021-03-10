
use std::borrow::Cow;
use smallvec::SmallVec;

use crate::iter_extensions::{ Completable, IterExt, };

/*
struct ParsingContext {
	posix: bool,
	/// If enabled, then doubly-quoted strings will not treat things as bad
	bash_hist_expansion: bool,
	ifs: Vec<u8>,
}
*/

// TODO: how to handle "nested" strings
// eg: command expansion like "hello, $(get_name)" 

/// Iterates over substrings that may follow different quoting styles
pub struct Substrings<'a> {
	rest: &'a [u8],
	//byte_start: usize,
}

impl<'a> Substrings<'a> {
	pub fn new(string: &'a [u8]) -> Substrings<'a> {
		Substrings {
			rest: string,
		}
	}

	/// Returns a result containing the next group of characters that is considered a field seperator
	///
	/// TODO: use IFS?
	fn consume_seperator(src: &[u8]) -> Option<UnescapeResult<'_>> {
		
		assert_eq!(QuoteStyle::starting_string_type(src)?, QuoteStyle::Separator, "attempt to consume non-separator string as separator");
		
		let last_consecutive_seperator = src.iter()
			.enumerate()
			.take_while(|(_, &b)| QuoteStyle::is_separator(b))
			.last();
		
		let (i, _) = last_consecutive_seperator?;

		Some(UnescapeResult {
			was_closed: true,
			unescaped: QuotedString::as_separator(&src[..=i]),
			rest: &src[i+1..],
		})
	}
	
	/// Consumes the next non-zero sized unquoted string, using backslashes to escape whitespace and dollar signs
	///
	/// If the string ends with a backslash, the backslash is *not* returned in the unescaped string, and the string is indicated as unclosed
	/// This allows an indication of two strings that should be concatenated, eg: two lines, with the first ending with backslash
	/// 
	fn consume_backslashed(src: &[u8]) -> Option<UnescapeResult<'_>> {
		// discussion: how to handle newlines?

		// also filters zero-length strings to None
		assert_eq!(QuoteStyle::starting_string_type(src)?, QuoteStyle::Backslash, "attempt to consume non-backslash/plain quoted string as backslash/plain quoted");

		// our working copy of the data
		let mut curr = Cow::Borrowed(src);
		// `inds.0 + base == curr[_]
		// the base of the current iterator into the curr string
		let mut base = 0;
		// our bytes within curr
		let mut inds = curr.iter().copied().enumerate().peekable();
		// used to double-check logic
		let mut removed = 0;

		
		// as\'\nss

		while let Some((i, b)) = inds.next() {
			// double-check our invariants
			assert_eq!(&curr[base+i..], &src[removed+base+i..], "rest of working string is different than rest of source string");

			// safety: if we entered this inner loop, then there should be characters in the string
			// if there are characters, then we'll have a string type
			// backslash quoted is unique in that we can switch to any type within the string - no need to match quotes or anything
			if QuoteStyle::starting_string_type(&src[removed+base+i..]).unwrap() != QuoteStyle::Backslash {
				// we are starting a different quoting style - exit this one
				// eprintln!("emitting result due to string type boundary...");
				let (res, rest) = truncate_cow_slice(src, curr, base+i, removed);
				
				return Some(UnescapeResult {
					was_closed: true,
					unescaped: QuotedString::as_backslash(res),
					rest: rest,
				});
			}

			if b == b'\\' {
				// next character is literal
				if inds.peek().is_none() {
					// we've reached the end of our source data
					// no need to truncate, rest.len() == 0
					let (res, rest) = truncate_cow_slice(src, curr, base+i, removed+1);
					assert_eq!(base+i, res.len());
					assert_eq!(0, rest.len());
					return Some(UnescapeResult {
						was_closed: false,
						unescaped: QuotedString::as_backslash(res), // curr
						rest: rest, // &src[base+i+1..],
					});
				} else {
					if base == 0 && i == 0 {
						assert_eq!(removed, 0);
						// by definition, base and i == 0, so will both be 1 after this
					}

					// skips the character after this current one
					std::mem::drop(inds);
					remove_cow_byte(&mut curr, base+i);
					removed += 1;
					base += i + 1;
					inds = curr[base..].iter().copied().enumerate().peekable();
				}
			}
		}

		// we reached the end of the string: we've consumed all the bytes
		assert_eq!(curr.len() + removed, src.len());
		Some(UnescapeResult {
			was_closed: true,
			rest: &src[curr.len() + removed..], // essentially &src[src.len()..].len() == 0
			unescaped: QuotedString::as_backslash(curr),
		})
	}

	/// Escapes singley-quoted strings.
	///
	/// The contents of singly-quoted strings are literal - no escapes. Ends at the next single-quote.
	///
	/// Will return None on a zero-length input, or if the string does not start with a single quote.
	fn consume_single(src: &[u8]) -> Option<UnescapeResult<'_>> {
		// also filters zero-length strings to None
		assert_eq!(QuoteStyle::starting_string_type(src)?, QuoteStyle::Single, "attempt to consume non-singly quoted string as singly quoted");

		let next_quote = src.iter()
			.enumerate()
			.skip(1) // skip the first quote
			.find(|(_, &b)| b == b'\'');
		
		match next_quote {
			Some((i, _)) => {
				Some(UnescapeResult {
					was_closed: true,
					unescaped: QuotedString::as_single(&src[1..i]),
					rest: &src[i+1..],
				})
			},
			None => {
				// no ending quote - we go until the end
				Some(UnescapeResult {
					was_closed: false,
					unescaped: QuotedString::as_single(&src[1..]),
					rest: &src[src.len()..],
				})
			}
		}
	}

	
	/// Consumes the next non-zero sized double-quoted string, using backslashes to escape dollar signs and backticks
	///
	/// The returned unescaped value may contain nested sub-strings for, eg, backticked data
	fn consume_double(src: &[u8]) -> Option<UnescapeResult<'_>> {
		// discussion: how to handle newlines?
		// TODO: POSIX conforming
		// eg: \! doesn't mean anything when in POSIX mode

		// also filters zero-length strings to None
		assert_eq!(QuoteStyle::starting_string_type(src)?, QuoteStyle::Double, "attempt to consume non-doubly quoted string as doubly quoted");
		assert_eq!(src.get(0), Some(&b'"'), "start of doubly-quoted string is not a double quote");

		struct SegmentResult<'a> {
			inner: InnerString<'a>,
			was_closed: bool,
			rest: &'a [u8],
		}

		/// Consumes a number of characters, processing backslash escapes appropriate for doubly-quoted strings.
		/// Stops either right before the end of the doubly-quoted string (that is, the next character is a double-quote)
		/// or at the beginning of an opening substring character(s) (backtick or "$(")
		///
		/// returns None if there are no characters left, or if the next character starts a substring
		fn consume_string_segment<'b>(src: &'b [u8]) -> Option<SegmentResult<'b>> {
			// our working copy of the data
			let mut curr = Cow::Borrowed(src);
			// `inds.0 + base == curr[_]
			// the base of the current iterator into the curr string
			let mut base = 0;
			// our bytes within curr
			let mut inds = curr.iter().copied().enumerate().peekable();
			// used to double-check logic
			let mut removed = 0;

			while let Some((i, b)) = inds.next() {
				// double-check our invariants
				assert_eq!(&curr[base+i..], &src[removed+base+i..], "rest of working string is different than rest of source string");

				match b {
					b'"' if base+i == 0 => { // no characters in this segment - close
						unreachable!("shouldn't enter consume_string_segment if next char is double-quote");
					}
					b'"' => { // close out string
						// return without consuming this character
						let (res, rest) = truncate_cow_slice(src, curr, base+i, removed);

						return Some(SegmentResult {
							was_closed: true,
							inner: InnerString::Segment(res),
							rest,
						});
					},
					b'`' => { // backtick denoted substring
						todo!("return - next char starts substring");
					},
					b'$' => {
						// should we handle variables? shell expansions? variable expansions?
						// $MY_VAR
						// ${MY_VAR:-OTHER}
						// $(my_command)

						todo!("doubly-quoted strings with expansions")
					},
					//b'$' if inds.peek().map(|&(_, b)| b == b'(').unwrap_or(false) => {
					//	todo!("return - next next char starts substring");
					//},
					b'\\' if inds.peek().is_none() => {
						// no char to escape - string not finished
						// leave the backslash for the outer-loop to detect and handle

						let (res, rest) = truncate_cow_slice(src, curr, base+i-1, removed);
						assert_eq!(base+i, res.len());
						assert_eq!(0, rest.len());

						return Some(SegmentResult {
							was_closed: false,
							inner: InnerString::Segment(res),
							rest,
						});
					},
					b'\\' => { // next char is literal
						if base == 0 && i == 0 {
							assert_eq!(removed, 0);
							// by definition, base and i == 0, so will both be 1 after this
						}

						// skips the character after this current one
						std::mem::drop(inds);
						assert!(remove_cow_byte(&mut curr, base+i));
						removed += 1;
						base += i + 1;
						inds = curr[base..].iter().copied().enumerate().peekable();
					}
					_ => {}, // regular characters
				}
			}

			// reached end of string with no more bytes
			// return - string not finished

			assert_eq!(curr.len() + removed, src.len());
			if curr.len() == 0 {
				None
			} else {
				// this segment has finished - even if there is no ending quote
				// the outer loop should detect the lack of ending quote and report as unfinished
				Some(SegmentResult {
					was_closed: true,
					rest: &src[curr.len() + removed..], // essentially &src[src.len()..].len() == 0
					inner: InnerString::Segment(curr),
				})
			}
		}
		fn consume_substring_segment<'b>(src: &'b [u8]) -> Option<SegmentResult<'b>> {
			let mut rest = src;
			let mut _inner_strings: Vec<UnescapeResult<'b>> = Vec::with_capacity(1);
			let (_style, closing) = match rest[0] {
				b'`' => (SubshellStyle::Backticks, b'`'),
				b'$' => (SubshellStyle::Legacy, b')'),
				_ => unreachable!(),
			};

			rest = &rest[1..];
			while rest.len() > 0 && rest[0] != closing {
				//
				todo!("process inner contents of substrings")
			}
			if rest.len() == 0 {
				// we did not find a closing
				todo!("return that string was unfinished");
			}

			todo!();
		}

		let mut rest = src;
		let mut segments: SmallVec<[InnerString<'_>; 1]> = SmallVec::new();

		// skip over opening quote
		rest = &rest[1..];
		while rest.len() > 0 {
			if rest.starts_with(b"$(") || rest[0] == b'`' { // substrings
				// substrings
				let _ = consume_substring_segment(rest).unwrap();
				todo!("doubly-quoted substrings");
				//let is = consume_substring_segment(rest);
				//segments.push(InnerString::Subshell(style, inner_strings));
			} else if rest[0] == b'"' { // end of string

				return Some(UnescapeResult {
					was_closed: true,
					rest: &rest[1..], // skip closing quote
					unescaped: QuotedString::Double(segments),
				});
			} else if rest.len() == 1 && rest[0] == b'\\' { // a segment found a backslash at the end of the input
				// rest will include the backslash
				return Some(UnescapeResult {
					was_closed: false,
					rest,
					unescaped: QuotedString::Double(segments),
				});
		 	} else { // regular string segment
				let ss = consume_string_segment(rest).unwrap();
				segments.push(ss.inner);
				rest = ss.rest;
				if ! ss.was_closed {
					// this should only not close on an escape character with no trailing character
					assert_eq!(rest.len(), 1);
					assert_eq!(rest[0], b'\\');

					// loop back around - we will trigger the specific case for that
				}
			}
		}

		// we reached the end of the string: we've consumed all the bytes without reaching an ending double quote
		//assert_eq!(curr.len() + removed, src.len());
		Some(UnescapeResult {
			was_closed: false,
			unescaped: QuotedString::Double(segments),
			rest,
		})
	}

	/// Consumes the rest of the iterator, and emits a string representing the unescaped and concatenated version of the contained strings
	fn join_all(&mut self) -> Cow<'_, str> {
		// can only return a Cow::Borrowed under the following conditions:
			// All strings were QuoteStyle::None (so only 1 string - otherwise there would be removed quotes)
			// There were no escaped characters within the string

		// join them all together
		todo!();
	}

	pub fn as_words_lossy(&'a mut self) -> (Vec<String>, bool) {
		let mut v: Vec<String> = Vec::new();
		let mut last_closed = true;
		for e in self {
			if let QuotedString::Separator(_) = e.unescaped { continue; }
			v.push(e.unescaped.to_string_lossy().to_string());
			last_closed = e.was_closed;
		}
		(v, last_closed)
	}
}
impl<'a> Iterator for Substrings<'a> {
	type Item = UnescapeResult<'a>;
	fn next(&mut self) -> Option<Self::Item> {
		let result = QuoteStyle::bash_unescape(self.rest)?;
		self.rest = result.rest;
		Some(result)
	}
}

// https://pubs.opengroup.org/onlinepubs/9699919799/utilities/V3_chap02.html#tag_18_06_05
struct Fields<'a> {
	ifs: Option<&'a [u8]>,
	backing: &'a [u8],
}


fn truncate_cow_slice<'a>(src: &'a [u8], c: Cow<'a, [u8]>, length: usize, removed: usize) -> (Cow<'a, [u8]>, &'a [u8]) {
	let res = match c {
		Cow::Borrowed(b) => Cow::Borrowed(&b[..length]),
		Cow::Owned(mut o) => {
			o.truncate(length);
			Cow::Owned(o)
		}
	};
	let rest = &src[length+removed..];

	/*eprintln!("\tsplitting {:?} into {:?} and {:?}",
		String::from_utf8_lossy(src),
		String::from_utf8_lossy(&res),
		String::from_utf8_lossy(&rest)
	);*/

	(res, rest)
}

/// removes a byte from a cow, preserving it as `Cow::Borrowed` when applicable if `at == 0`
///
/// returns `true` if a byte was removed
fn remove_cow_byte(data: &mut Cow<'_, [u8]>, at: usize) -> bool {
	if data.len()-1 < at { return false; }

	match data {
		Cow::Borrowed(b) if at == 0 => {
			*data = Cow::Borrowed(&b[1..]);
		},
		_ => {
			data.to_mut().remove(at);
		}
	}
	
	true
}



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubshellStyle {
	/// ```...```
	Backticks,
	/// `$(...)`
	Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InnerString<'a> {
	Segment(Cow<'a, [u8]>),
	Subshell(SubshellStyle, Vec<UnescapeResult<'a>>),
}
impl<'a> InnerString<'a> {
	/// Determines if the objects are not only equal, but any Cow objects' allocated states are equal as well
	#[cfg(test)]
	fn equal_allocated(&self, o: &Self) -> bool {
		if self != o { return false; }

		match self {
			InnerString::Segment(s) => {
				match o {
					InnerString::Segment(o) => {
						//return s.is_borrowed() == o.is_borrowed();
						return match (s, o) {
							(Cow::Borrowed(_), Cow::Borrowed(_)) => true,
							(Cow::Owned(_), Cow::Owned(_)) => true,
							_ => false,
						};
					},
					_ => unreachable!(),
				}
			},
			InnerString::Subshell(_, v) => {
				match o {
					InnerString::Subshell(_, o) => {
						return v.iter().zip(o)
							.all(|(a, b)| a.unescaped.equal_allocated(&b.unescaped));
					},
					_ => unreachable!(),
				}
			}
		}
	}
}

mod escape_string {
	use super::*;

	pub fn as_backslash(mut s: Cow<'_, [u8]>) -> Cow<'_, [u8]> {
		// TODO: change definitions of stuff so we can use the original string if we want

		// must escape separator characters, and specially interpreted characters

		let mut i = 0;
		while i < s.len() {
			if QuoteStyle::is_separator(s[i]) || matches!(s[i], b'\\' | b'*' | b'\'' | b'"' | b'$' | b'!' | b'`' | b'(' | b')' | b';' | b'|' | b'&') {
				s.to_mut().insert(i, b'\\');
				i += 1;
			}
			i += 1;
		}

		s
	}

	// this will always allocate: no way to add characters without allocating
	pub fn as_single(mut s: Vec<u8>, close_end: bool) -> Vec<u8> {
		s.insert(0, b'\'');
		if close_end {
			s.push(b'\'');
		}
		s
	}
}

// Stores unescaped byte strings, ready to be viewed, or escaped
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotedString<'a> {
	// A field seperator.
	Separator(Cow<'a, [u8]>),
	/// Unquoted strings using backslashes to escape whitespace and dollar signs.
	Backslash(Cow<'a, [u8]>),
	/// A string used to encode literal contents.
	Single(Cow<'a, [u8]>),
	/// Double-quote style escape strings. Must escape dollar signs if used literally.
	///
	/// May contain a shell-expandable string if unescaped backticks are used.
	Double(SmallVec<[InnerString<'a>; 1]>),
	/// Allows the user to specify ANSI escape codes (\n \t etc)
	AnsiC(Cow<'a, [u8]>), // $'asd'
	/// A string whose encoding is interpreted according to the shell's locale
	///
	/// May contain a shell-expandable string if unescaped backticks are used.
	Locale(SmallVec<[InnerString<'a>; 1]>), // $"asd"
}
impl<'a> QuotedString<'a> {
	/// Interprets the passed value as a seperator string. Does not check if the contained characters are valid seperators.
	pub fn as_separator<T: Into<Cow<'a, [u8]>>>(c: T) -> QuotedString<'a> {
		QuotedString::Separator(c.into())
	}
	pub fn as_backslash<T: Into<Cow<'a, [u8]>>>(c: T) -> QuotedString<'a> {
		QuotedString::Backslash(c.into())
	}
	pub fn as_single<T: Into<Cow<'a, [u8]>>>(c: T) -> QuotedString<'a> {
		QuotedString::Single(c.into())
	}
	pub fn as_double<T: Into<Cow<'a, [u8]>>>(c: T) -> QuotedString<'a> {
		let mut sv = SmallVec::new();
		sv.push(InnerString::Segment(c.into()));
		QuotedString::Double(sv)
	}
	pub fn as_double_compound<T: Into<Cow<'a, [u8]>>>(c: T) -> QuotedString<'a> {
		todo!("double strings as compound")
	}
	pub fn as_ansi_c<T: Into<Cow<'a, [u8]>>>(c: T) -> QuotedString<'a> {
		QuotedString::AnsiC(c.into())
	}
	pub fn as_locale<T: Into<Cow<'a, [u8]>>>(c: T) -> QuotedString<'a> {
		let mut sv = SmallVec::new();
		sv.push(InnerString::Segment(c.into()));
		QuotedString::Locale(sv)
	}
	pub fn as_locale_compound<T: Into<Cow<'a, [u8]>>>(c: T) -> QuotedString<'a> {
		todo!("locale strings as compound")
	}

	pub fn style(&self) -> QuoteStyle {
		match self {
			QuotedString::Separator(_) => QuoteStyle::Separator,
			QuotedString::Backslash(_) => QuoteStyle::Backslash,
			QuotedString::Single(_) => QuoteStyle::Single,
			QuotedString::Double(_) => QuoteStyle::Double,
			QuotedString::AnsiC(_) => QuoteStyle::AnsiC,
			QuotedString::Locale(_) => QuoteStyle::Locale,
		}
	}

	pub fn to_string_lossy(&self) -> Cow<'_, str> {
		use QuotedString::*;
		match self {
			Separator(s) | Backslash(s) | Single(s) | AnsiC(s) => {
				String::from_utf8_lossy(s)
			},
			Double(v) | Locale(v) => {
				match &v.as_slice() {
					// hopefully, many of these have no subshells == 1 segment
					&[InnerString::Segment(s)] => String::from_utf8_lossy(&s),
					ve @ _ => {
						let mut s = String::new();
						for e in *ve {
							match e {
								InnerString::Segment(st) => s.push_str(&String::from_utf8_lossy(st)),
								InnerString::Subshell(st, ss) => {
									let mut is = String::new();
									for s in ss {
										is.push_str(&s.unescaped.to_string_lossy());
									}

									match st {
										SubshellStyle::Backticks => s.push_str(&format!("`{:?}`", &is)),
										SubshellStyle::Legacy => s.push_str(&format!("$({:?})", &is)),
									}
								}
							}
						}
						Cow::Owned(s)
					}
				}
				//unimplemented!("to string of double/locale strings")
			}
		}
	}

	pub fn escape(self, close_end: bool) -> Cow<'a, [u8]> {
		match self {
			QuotedString::Separator(s) => s,
			QuotedString::Backslash(s) => escape_string::as_backslash(s),
			QuotedString::Single(s) => Cow::Owned(escape_string::as_single(s.to_vec(), close_end)),
			_ => todo!("other quoted string types"),
		}
	}

	/// Determines if the objects are not only equal, but any Cow objects' allocated states are equal as well
	#[cfg(test)]
	fn equal_allocated(&self, o: &Self) -> bool {
		use QuotedString::*;

		// short-circuit on built in
		if self != o { return false; }

		match self {
			Separator(s) | Backslash(s) | Single(s) | AnsiC(s) => {
				match o {
					Double(_) | Locale(_) => unreachable!("mismatched enum variants after checking equality"),
					Separator(o) | Backslash(o) | Single(o) | AnsiC(o) => {
						//return s.is_borrowed() == o.is_borrowed();
						return match (s, o) {
							(Cow::Borrowed(_), Cow::Borrowed(_)) => true,
							(Cow::Owned(_), Cow::Owned(_)) => true,
							_ => false,
						};
					}
				}
			},
			Double(sv) | Locale(sv) => {
				match o {
					Separator(_) | Backslash(_) | Single(_) | AnsiC(_) => unreachable!("mismatched enum variants after checking equality"),
					Double(ov) | Locale(ov) => {
						return sv.iter().zip(ov)
							.all(|(a, b)| a.equal_allocated(&b));
					}
				}
			},
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnescapeResult<'a> {
	/// The quote style of the consumed string
	//style: QuoteStyle,

	/// The unescaped string
	//unescaped: Cow<'a, S>,
	pub unescaped: QuotedString<'a>,

	/// If the string found an appropriate terminating character (if QuoteStyle::Backslash, then only false if last character is backslash)
	pub was_closed: bool,

	/// The rest of the unconsumed string
	pub rest: &'a [u8],
}
impl<'a> UnescapeResult<'a> {
	pub fn to_string_lossy(&self) -> (Cow<'_, str>, QuoteStyle, bool) {
		(
			self.unescaped.to_string_lossy(),
			self.unescaped.style(),
			self.was_closed,
		)
	}
}

// rename to word type?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteStyle {
	// A field seperator.
	Separator,
	/// Unquoted strings using backslashes to escape whitespace and dollar signs.
	Backslash,
	/// A string used to encode literal contents.
	Single,
	/// Double-quote style escape strings. Must escape dollar signs if used literally.
	///
	/// May contain a shell-expandable string if unescaped backticks are used.
	Double,
	/// Allows the user to specify ANSI escape codes (\n \t etc)
	AnsiC, // $'asd'
	/// A string whose encoding is interpreted according to the shell's locale
	///
	/// May contain a shell-expandable string if unescaped backticks are used.
	Locale, // $"asd"
}

// note that the shell encoding is defined in env var LC_CTYPE

impl QuoteStyle {
	
	fn is_separator<C: Into<char>>(c: C) -> bool {
		// https://mywiki.wooledge.org/BashGuide/SpecialCharacters
		const VERTICAL_TAB: char = 0x0B as char;
		const FORM_FEED: char = 0x0C as char;
		matches!(c.into(), '\t' | '\n' | VERTICAL_TAB | FORM_FEED | '\r' | ' ')
	}

	/// Determines the type of quote style that the provided byte slice starts with. 
	///
	/// Will return None on a zero-sized slice
	pub fn starting_string_type(b: &[u8]) -> Option<QuoteStyle> {
		if b.len() == 0 { None }
		else if QuoteStyle::is_separator(b[0]) { Some(QuoteStyle::Separator) }
		else if b.starts_with(b"'") { Some(QuoteStyle::Single) }
		else if b.starts_with(b"\"") { Some(QuoteStyle::Double) }
		else if b.starts_with(b"$'") { Some(QuoteStyle::AnsiC) }
		else if b.starts_with(b"$\"") { Some(QuoteStyle::Locale) }
		
		// includes backslash
		else { Some(QuoteStyle::Backslash) }
	}

	/// Stores the unescaped byte string into a QuotedString struct based on the variant of the current QuoteStyle
	pub fn wrap_raw<'a, T: Into<Cow<'a, [u8]>>>(&'_ self, b: T) -> QuotedString<'a> {
		use QuoteStyle::*;
		let b = b.into();
		match self {
			Separator => QuotedString::as_separator(b),
			Backslash => QuotedString::as_backslash(b),
			Single => QuotedString::as_single(b),
			Double => QuotedString::as_double(b),
			AnsiC => QuotedString::as_ansi_c(b),
			Locale => QuotedString::as_locale(b),
		}
	}

	// tries to unescape the string using the provided quote style
	pub fn bash_unescape_as<'a>(&self, s: &'a [u8]) -> Option<UnescapeResult<'a>> {
		match self {
			QuoteStyle::Separator => Substrings::consume_seperator(s),
			QuoteStyle::Backslash => Substrings::consume_backslashed(s),
			QuoteStyle::Single => Substrings::consume_single(s),
			QuoteStyle::Double => Substrings::consume_double(s),
			QuoteStyle::AnsiC => todo!("Unescaping strings with AnsiC style quotes are not currently supported."),
			QuoteStyle::Locale => todo!("Unescaping strings with Locale style quotes are not currently supported."),
		}
	}
	
	// auto-detects the string type, and starts unescaping the string as such
	pub fn bash_unescape<'a>(s: &'a [u8]) -> Option<UnescapeResult<'a>> {
		QuoteStyle::starting_string_type(s)?.bash_unescape_as(s)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	type TestResult<'a> = (usize, QuotedString<'a>);
	fn run_compact_cases<'a>(test_case: &'a [u8], expected: &'_ [TestResult<'a>], last_closed: bool) {
		let mut generated = Substrings::new(test_case);
		let mut rest_ind = 0;
		for (i, exp) in expected.iter().enumerate() {
			rest_ind += exp.0;
			let _exp_str = exp.1.to_string_lossy();
			let _exp_rest = String::from_utf8_lossy(&test_case[rest_ind..]);

			let gen = generated.next().expect(&format!("did not generate at least {} elements as expected", i+1));

			let _gen_str = gen.unescaped.to_string_lossy();
			let _gen_rest = String::from_utf8_lossy(&gen.rest);
			
			assert_eq!(exp.1, gen.unescaped, "word {} was not as expected", i);
			assert_eq!(&test_case[rest_ind..], gen.rest, "rest for word {} was not as expected", i);
			assert!(exp.1.equal_allocated(&gen.unescaped), "word {} was incorrectly allocated", i);

			if i == expected.len() {
				assert_eq!(last_closed, gen.was_closed, "word {} was not correctly marked as (un)closed", i);
			}
		}
		assert_eq!(None, generated.next(), "incorrectly generated extra results");
	}

	/// Tests seperating strings with out needing to escape characters/modify the string
	#[test]
	fn bash_separate_plain_fields() {
		const TEST_CASE: &'static [u8] = ("bash \nparsing  is\t neat").as_bytes();

		const EXPECTED: &[TestResult] = &[
			(4, QuotedString::Backslash(Cow::Borrowed(&*b"bash"))),
			(2, QuotedString::Separator(Cow::Borrowed(&*b" \n"))),
			(7, QuotedString::Backslash(Cow::Borrowed(&*b"parsing"))),
			(2, QuotedString::Separator(Cow::Borrowed(&*b"  "))),
			(2, QuotedString::Backslash(Cow::Borrowed(&*b"is"))),
			(2, QuotedString::Separator(Cow::Borrowed(&*b"\t "))),
			(4, QuotedString::Backslash(Cow::Borrowed(&*b"neat"))),
		];

		run_compact_cases(&TEST_CASE, &EXPECTED, true);
	}

	/// Tests escaping characters with backslash escapes
	#[test]
	fn bash_quote_backslash() {
		const TEST_CASE: &'_ [u8] = ("bash par\\ sing is\\$ \\$pretty \\$n\\$eat see\\").as_bytes();

		let expected: &[TestResult] = &[
			(4, QuotedString::Backslash(Cow::Borrowed(&*b"bash"))),
			(1, QuotedString::Separator(Cow::Borrowed(&*b" "))),
			(9, QuotedString::Backslash(Cow::Owned(b"par sing".to_vec()))),
			(1, QuotedString::Separator(Cow::Borrowed(&*b" "))),
			(4, QuotedString::Backslash(Cow::Owned(b"is$".to_vec()))),
			(1, QuotedString::Separator(Cow::Borrowed(&*b" "))),
			(8, QuotedString::Backslash(Cow::Borrowed(&*b"$pretty"))),
			(1, QuotedString::Separator(Cow::Borrowed(&*b" "))),
			(8, QuotedString::Backslash(Cow::Owned(b"$n$eat".to_vec()))),
			(1, QuotedString::Separator(Cow::Borrowed(&*b" "))),
			(4, QuotedString::Backslash(Cow::Borrowed(&*b"see"))),
		];

		run_compact_cases(&TEST_CASE, &expected, false);
	}

	#[test]
	fn bash_quote_single() {
		const TEST_CASE: &'_ [u8] = ("'''asd'  '\\n' '$s''' '''").as_bytes();

		const EXPECTED: &[TestResult] = &[
			(2, QuotedString::Single(Cow::Borrowed(&*b""))),
			(5, QuotedString::Single(Cow::Borrowed(&*b"asd"))),
			(2, QuotedString::Separator(Cow::Borrowed(&*b"  "))),
			(4, QuotedString::Single(Cow::Borrowed(&*b"\\n"))),
			(1, QuotedString::Separator(Cow::Borrowed(&*b" "))),
			(4, QuotedString::Single(Cow::Borrowed(&*b"$s"))),
			(2, QuotedString::Single(Cow::Borrowed(&*b""))),
			(1, QuotedString::Separator(Cow::Borrowed(&*b" "))),
			(2, QuotedString::Single(Cow::Borrowed(&*b""))),
			(1, QuotedString::Single(Cow::Borrowed(&*b""))),
		];

		run_compact_cases(&TEST_CASE, &EXPECTED, false);
	}


	#[test]
	fn bash_quote_double() {
		// TODO: add substrings to this, especially with double-quoted strings inside it
		const TEST_CASE: &'_ [u8] = (r#"start"mid"end "surround" "escap\e\"" "unfinish"#).as_bytes();

		let expected: &[TestResult] = &[
			(5, QuotedString::Backslash(Cow::Borrowed(&*b"start"))),
			(5, QuotedString::Double(SmallVec::from_buf([InnerString::Segment(Cow::Borrowed(&*b"mid"))]))),
			(3, QuotedString::Backslash(Cow::Borrowed(&*b"end"))),
			(1, QuotedString::Separator(Cow::Borrowed(&*b" "))),
			(10, QuotedString::Double(SmallVec::from_buf([InnerString::Segment(Cow::Borrowed(&*b"surround"))]))),
			(1, QuotedString::Separator(Cow::Borrowed(&*b" "))),
			(11, QuotedString::Double(SmallVec::from_buf([InnerString::Segment(Cow::Owned(b"escape\"".to_vec()))]))),
			(1, QuotedString::Separator(Cow::Borrowed(&*b" "))),
			(9, QuotedString::Double(SmallVec::from_buf([InnerString::Segment(Cow::Borrowed(&*b"unfinish"))]))),
		];

		run_compact_cases(&TEST_CASE, expected, false);
	}

	#[test]
	fn bash_doubly_quoted_substrings() {
		// TODO
	}

	/// Tests a series of consecutive differently quoted strings, and that they are able to be viewed without allocating.
	#[test]
	fn parse_plain_consecutive_strings_all_types() {
		const TEST_CASE: &'_ [u8] = (r#"he's'$'s'll"world"''$"asd"o"#).as_bytes();

		let expected: &[TestResult] = &[
			(2, QuotedString::Backslash(Cow::Borrowed(&*b"he"))),
			(3, QuotedString::Single(Cow::Borrowed(&*b"s"))),
			(2, QuotedString::AnsiC(Cow::Borrowed(&*b"s"))),
			(2, QuotedString::Backslash(Cow::Borrowed(&*b"ll"))),
			(7, QuotedString::Double(SmallVec::from_buf([InnerString::Segment(Cow::Borrowed(&*b"world"))]))),
			(2, QuotedString::Single(Cow::Borrowed(&*b""))),
			(6, QuotedString::Locale(SmallVec::from_buf([InnerString::Segment(Cow::Borrowed(&*b"asd"))]))),
			(2, QuotedString::Backslash(Cow::Borrowed(&*b"o"))),
		];

		run_compact_cases(&TEST_CASE, expected, true);
	}


}

