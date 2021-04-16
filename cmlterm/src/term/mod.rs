

use std::fmt::Debug;
use futures::stream::FusedStream;
use futures::{Sink, Stream};

/// A stream/sink pair suitable for interfacing with a console line with.
///
/// The resulting stream output should be directly from the console, stripped of any backend-specific data (such as "trying connection..." or "connected" text)
///
/// Any data sent in should be sent as directly to the console as possible
pub trait Drivable<E: Send + Sync + std::fmt::Debug + std::error::Error + 'static>: Debug + Unpin + FusedStream + Stream<Item = Result<Vec<u8>, E>> + Sink<String, Error = E> {}
impl<T, E: Send + Sync + std::fmt::Debug + std::error::Error + 'static> Drivable<E> for T where T: Debug + Unpin + FusedStream + Stream<Item = Result<Vec<u8>, E>> + Sink<String, Error = E> {}

pub mod backend;
pub mod common;
pub mod frontend;

pub use common::{ ConsoleDriver, ConsoleUpdate };
