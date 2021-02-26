
use std::borrow::Cow;
use std::fmt::Debug;
use super::QuoteStyle;



pub trait IterExt {
	fn flatten_tuple2<T>(self) -> FlattenTuple2<Self, T>
	where
		Self: Sized + Iterator<Item = (T, T)>
	{
		FlattenTuple2 {
			iter: self,
			queue: None,
		}
	}

	/// Returns a list of completions based on the current iterator.
	///
	/// Filters based on the passed-in current word, and optionally finishes the quoted style.
	fn quote_matches<'cword, 'item, C>(self, base: &'cword C, quote_style: QuoteStyle, finish_word: bool) -> QuoteWord<'item, C, StartsWith<'cword, 'item, C, Self>>
	where
		Self: Iterator + Sized,
		<Self as Iterator>::Item: Into<Cow<'item, C>> + AsRef<C>,
		C: Completable + ?Sized + Debug
	{
		QuoteWord {
			iter: StartsWith {
				iter: self,
				base,
				phantom: std::marker::PhantomData,
			},
			quote_style,
			finish_word,
			phantom: std::marker::PhantomData,
		}
	}

	fn filter_matches<'cword, 'item, C>(self, base: &'cword C) -> StartsWith<'cword, 'item, C, Self>
	where
		Self: Iterator + Sized,
		<Self as Iterator>::Item: AsRef<C>,
		C: Completable + ?Sized
	{
		StartsWith {
			iter: self,
			base,
			phantom: std::marker::PhantomData,
		}
	}

	fn quote_word<'item, C>(self, quote_style: QuoteStyle, finish_word: bool) -> QuoteWord<'item, C, Self>
	where
		Self: Iterator + Sized,
		<Self as Iterator>::Item: Into<Cow<'item, C>>,
		C: Completable + ?Sized
	{
		QuoteWord {
			iter: self,
			quote_style,
			finish_word,
			phantom: std::marker::PhantomData,
		}
	}
}
//impl<T, I: Iterator<Item = (T, T)>> IterExt for I {}
pub struct FlattenTuple2<I: Iterator<Item = (T, T)>, T> {
	iter: I,
	queue: Option<T>,
}
impl<T, I: Iterator<Item = (T, T)>> Iterator for FlattenTuple2<I, T> {
	type Item = T;
	fn next(&mut self) -> Option<T> {
		if self.queue.is_none() {
			let (a, b) = self.iter.next()?;
			self.queue = Some(b);
			Some(a)
		} else {
			self.queue.take()
		}
	}
}


/// Types that are able to be shell-completed, matched against others, then quoted for output
pub trait Completable: ToOwned {
	fn matches_cword(&self, cword: &Self) -> bool;
	fn wrap(item: Cow<Self>, qs: QuoteStyle, finish: bool) -> Cow<Self>;
}
impl Completable for str {
	fn matches_cword(&self, cword: &Self) -> bool {
		cword.len() == 0 || self.starts_with(cword)
	}
	fn wrap(item: Cow<Self>, qs: QuoteStyle, finish: bool) -> Cow<Self> {
		// Forward to the [u8] implementation, decoding/encoding from UTF8 as appropriate
		// SAFETY: escaping data will never introduce non-UTF8 bytes
		let as_bytes: Cow<[u8]> = match item {
			Cow::Borrowed(b) => Cow::Borrowed(b.as_ref()),
			Cow::Owned(o) => Cow::Owned(o.into_bytes()),
		};
		let wrapped = <[u8] as Completable>::wrap(as_bytes, qs, finish);
		match wrapped {
			Cow::Borrowed(b) => Cow::Borrowed(std::str::from_utf8(b).expect("input was UTF8, output should be UTF8 too")),
			Cow::Owned(b) => Cow::Owned(String::from_utf8(b).expect("input was UTF8, output should be UTF8 too")),
		}
	}
}
impl Completable for [u8] {
	fn matches_cword(&self, cword: &Self) -> bool {
		cword.len() == 0 || self.starts_with(cword)
	}
	fn wrap(item: Cow<Self>, qs: QuoteStyle, finish: bool) -> Cow<Self> {
		qs.wrap_raw(item).escape(finish)
	}
}

// TODO:
// impl Completable for OsStr ???


impl<I: Iterator + Sized> IterExt for I {
	fn filter_matches<'cword, 'item, C>(self, base: &'cword C) -> StartsWith<'cword, 'item, C, Self>
	where
		Self: Iterator + Sized,
		<Self as Iterator>::Item: AsRef<C>,
		C: Completable + ?Sized
	{
		StartsWith {
			iter: self,
			base,
			phantom: std::marker::PhantomData,
		}
	}

	fn quote_word<'item, C>(self, quote_style: QuoteStyle, finish_word: bool) -> QuoteWord<'item, C, Self>
	where
		Self: Iterator + Sized,
		<Self as Iterator>::Item: Into<Cow<'item, C>>,
		C: Completable + ?Sized
	{
		QuoteWord {
			iter: self,
			quote_style,
			finish_word,
			phantom: std::marker::PhantomData,
		}
	}
}

pub struct StartsWith<'cword, 'item, C, I>
where
	I: Iterator,
	I::Item: AsRef<C>,
	C: Completable + ?Sized + 'item,
{
	iter: I,
	base: &'cword C,
	phantom: std::marker::PhantomData<&'item ()>,
}
pub struct QuoteWord<'item, C, I>
where
	I: Iterator,
	I::Item: Into<Cow<'item, C>>,
	C: Completable + ?Sized + 'item,
{
	iter: I,
	quote_style: QuoteStyle,
	finish_word: bool,
	phantom: std::marker::PhantomData<&'item C>,
}

impl<'cword, 'item, I, C> Iterator for StartsWith<'cword, 'item, C, I>
where
	I: Iterator,
	I::Item: AsRef<C>,
	C: Completable + ?Sized + 'item + Debug,
{
	type Item = I::Item;
	fn next(&mut self) -> Option<Self::Item> {
		// cycle entries until we hit a match
		// this prevents returning None and prematurely ending the Iterator

		while let Some(ref_c) = self.iter.next() {
			let item = ref_c.as_ref();
			if item.matches_cword(self.base) {
				return Some(ref_c);
			}
		}

		None
	}
}

impl<'item, I, C> Iterator for QuoteWord<'item, C, I>
where
	I: Iterator,
	I::Item: Into<Cow<'item, C>>,
	C: Completable + ?Sized + 'item + Debug,
{
	type Item = Cow<'item, C>;
	fn next(&mut self) -> Option<Self::Item> {
		let into_cow = self.iter.next()?;
		let item = into_cow.into();
		let as_cow_str = C::wrap(item, self.quote_style, self.finish_word);
		Some(as_cow_str)
	}
}