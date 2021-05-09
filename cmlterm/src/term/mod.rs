
use std::cell::{BorrowError, BorrowMutError, Ref, RefCell};
use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;
use futures::stream::FusedStream;
use futures::{ Sink, Stream};

/// A stream/sink pair suitable for interfacing with a console line with.
///
/// The resulting stream output should be directly from the console, stripped of any backend-specific data (such as "trying connection..." or "connected" text)
///
/// Any data sent in should be sent as directly to the console as possible
pub trait Drivable: Debug + Unpin + FusedStream + Stream<Item = Result<Vec<u8>, BoxedDriverError>> + Sink<String, Error = BoxedDriverError> {}
impl<T> Drivable for T where T: Debug + Unpin + FusedStream + Stream<Item = Result<Vec<u8>, BoxedDriverError>> + Sink<String, Error = BoxedDriverError> {}

pub type BoxedDriverError = anyhow::Error;


mod rw_meta;
pub use rw_meta::{UserTUI, IndepIsTty};
//#[cfg(unix)] pub use rw_meta::raw_fd::IndepAsRawFd;
//#[cfg(windows)] pub use rw_meta::raw_handle::IndepAsRawHandle;

pub mod backend;
pub mod common;
pub mod frontend;

pub use common::{ ConsoleDriver, ConsoleUpdate };

#[derive(Debug)]
struct InnerKeyedCache {
	cache: VecDeque<u8>,
	seq: usize,
}

/// A rotating byte cache, that can provide references to an sequenced view of the buffer, these references will be invalidated if the buffer is updated.
#[derive(Debug)]
pub struct KeyedCache(Arc<RefCell<InnerKeyedCache>>);
impl KeyedCache {
	fn with_capacity(capacity: usize) -> KeyedCache {
		KeyedCache(Arc::new(RefCell::new(InnerKeyedCache {
			cache: VecDeque::with_capacity(capacity),
			seq: 0,
		})))
	}

	fn try_update(&self, chunk: &[u8]) -> Result<KeyedCacheRef, BorrowMutError> {
		let seq = {
			let mut kc = self.0.try_borrow_mut()?;
			crate::update_chunkbuf(&mut kc.cache, chunk);
			kc.seq += 1;
			kc.seq
		};
		

		Ok(KeyedCacheRef {
			cache: Arc::clone(&self.0),
			seq,
		})
	}
}

#[derive(Debug, Error)]
pub enum KeyedCacheError {
	#[error("Attempt to receive keyed cache entry when it is mutably borrowed")]
	AlreadyBorrowed(#[from] BorrowError),
	#[error("Current key has an invalid (outdated?) sequence number. (expected = {}, current = {})", .expected, .current)]
	BadSequence { expected: usize, current: usize },
}

#[derive(Debug, Clone)]
pub struct KeyedCacheRef {
	cache: Arc<RefCell<InnerKeyedCache>>,
	seq: usize,
}
impl KeyedCacheRef {
	pub fn try_borrow(&self) -> Result<Ref<[u8]>, KeyedCacheError> {
		let cache = self.cache.try_borrow()?;
		if cache.seq != self.seq {
			return Err(KeyedCacheError::BadSequence {
				expected: self.seq,
				current: cache.seq,
			});
		}

		Ok(Ref::map(cache, |c| c.cache.as_slices().0))
	}
}

