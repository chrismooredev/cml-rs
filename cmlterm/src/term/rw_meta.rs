
use std::cell::{BorrowError, BorrowMutError, Ref, RefCell};
use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::Arc;
use crossterm::tty::IsTty;
use merge_io::MergeIO;
use thiserror::Error;
use futures::stream::FusedStream;
use futures::{AsyncRead as FAsyncRead, AsyncWrite as FAsyncWrite, Sink, Stream};
/*
mod plat {
	#[cfg(unix)] pub use std::os::unix::io::{AsRawFd, RawFd};
	#[cfg(windows)] pub use std::os::windows::io::{AsRawHandle, RawHandle};
}
use plat::*;

#[cfg(unix)] pub(crate) mod raw_fd {
	use super::*;

	pub trait IndepAsRawFd {
		fn as_raw_fd_reader(&self) -> RawFd;
		fn as_raw_fd_writer(&self) -> RawFd;
	}
	
	impl<R: AsRawFd + FAsyncRead + Unpin, W: AsRawFd + FAsyncWrite + Unpin> IndepAsRawFd for MergeIO<R, W> {
		fn as_raw_fd_reader(&self) -> RawFd { self.reader().as_raw_fd() }
		fn as_raw_fd_writer(&self) -> RawFd { self.writer().as_raw_fd() }
	}
}

#[cfg(windows)] pub(crate) mod raw_handle {
	use super::*;

	pub trait IndepAsRawHandle {
		fn as_raw_handle_reader(&self) -> RawHandle;
		fn as_raw_handle_writer(&self) -> RawHandle;
	}
	impl<R: AsRawHandle + FAsyncRead + Unpin, W: AsRawHandle + FAsyncWrite + Unpin> IndepAsRawHandle for MergeIO<R, W> {
		fn as_raw_handle_reader(&self) -> RawHandle { self.reader().as_raw_handle() }
		fn as_raw_handle_writer(&self) -> RawHandle { self.writer().as_raw_handle() }
	}
}*/

pub trait IndepIsTty {
	type Reader: IsTty;
	type Writer: IsTty;

	fn is_tty_reader(&self) -> bool;
	fn is_tty_writer(&self) -> bool;
}

impl<R: IsTty + FAsyncRead + Unpin, W: IsTty + FAsyncWrite + Unpin> IndepIsTty for MergeIO<R, W> {
	type Reader = R;
	type Writer = W;

	fn is_tty_reader(&self) -> bool { self.reader().is_tty() }
	fn is_tty_writer(&self) -> bool { self.writer().is_tty() }
}
/*
#[cfg(unix)]
impl<T: raw_fd::IndepAsRawFd, R: AsRawFd, W: AsRawFd> IndepIsTty<R, W> for T {
	fn is_tty_reader(&self) -> bool { self.as_raw_fd_reader().is_tty() }
	fn is_tty_writer(&self) -> bool  { self.as_raw_fd_writer().is_tty() }
}*/
/*#[cfg(unix)]
impl<T: AsRawFd> IndepIsTty<T, T> for T {
	fn is_tty_reader(&self) -> bool { self.is_tty() }
	fn is_tty_writer(&self) -> bool  { self.is_tty() }
}*/
/*
#[cfg(windows)]
impl<T: raw_handle::IndepAsRawHandle, R: AsRawHandle, W: AsRawHandle> IndepIsTty<R, W> for T {
	fn is_tty_reader(&self) -> bool { self.as_raw_handle_reader().is_tty() }
	fn is_tty_writer(&self) -> bool  { self.as_raw_handle_writer().is_tty() }
}*/


pub trait UserTUI:
	FAsyncRead + FAsyncWrite + Unpin +
	IndepIsTty<Reader = Self::Read, Writer = Self::Write>
{
	type Read: FAsyncRead + Unpin + IsTty;
	type Write: FAsyncWrite + Unpin + IsTty;

	fn reader(&self) -> &Self::Read;
	fn writer(&self) -> &Self::Write;
}
/*
impl<T: FAsyncRead + FAsyncWrite + Unpin + IndepIsTty<T, T>> UserTUI for T {
	type Reader = T;
	type Writer = T;
}*/


impl<R, W> UserTUI for MergeIO<R, W>
where
	R: FAsyncRead + Unpin + IsTty,
	W: FAsyncWrite + Unpin + IsTty,
{
	type Read = R;
	type Write = W;

	fn reader(&self) -> &Self::Read { self.reader() }
	fn writer(&self) -> &Self::Write { self.writer() }
}


/*
pub trait UserTUI: FAsyncRead + FAsyncWrite + Unpin + IndepIsTty<Self::Reader, Self::Writer> {
	type Reader: FAsyncRead + Unpin + IsTty;
	type Writer: FAsyncWrite + Unpin + IsTty;

	fn into_reader(self) -> Self::Reader;
	fn into_writer(self) -> Self::Writer;

	/// Provides access to `reader`.
	fn reader(&self) -> &Self::Reader;

	/// Provides access to `writer`.
	fn writer(&self) -> &Self::Writer;

	/// Provides `mut` access to `reader`.
	fn reader_mut(&mut self) -> &mut Self::Reader;

	/// Provides `mut` access to `writer`.
	fn writer_mut(&mut self) -> &mut Self::Writer;

	/// Deconstructs this struct into separate `reader` and `writer` halves
	fn into_inner(self) -> (Self::Reader, Self::Writer);
}

impl<R, W> UserTUI for MergeIO<R, W>
where
	R: FAsyncRead + Unpin + IsTty,
	W: FAsyncWrite + Unpin + IsTty,
{
	type Reader = R;
	type Writer = W;

	fn reader(&self) -> &Self::Reader {
		MergeIO::reader(self)
	}

	fn writer(&self) -> &Self::Writer {
		MergeIO::writer(self)
	}

	fn reader_mut(&mut self) -> &mut Self::Reader {
		MergeIO::reader_mut(self)
	}

	fn writer_mut(&mut self) -> &mut Self::Writer {
		MergeIO::writer_mut(self)
	}

	fn into_inner(self) -> (Self::Reader, Self::Writer) {
		MergeIO::into_inner(self)
	}
}

impl UserTUI for tokio_util::compat::Compat<TcpStream> {
	type Reader = ReadHalf<Self>;
	type Writer = WriteHalf<Self>;

	fn reader(&self) -> &Self::Reader {
		MergeIO::reader(self)
	}

	fn writer(&self) -> &Self::Writer {
		MergeIO::writer(self)
	}

	fn reader_mut(&mut self) -> &mut Self::Reader {
		MergeIO::reader_mut(self)
	}

	fn writer_mut(&mut self) -> &mut Self::Writer {
		MergeIO::writer_mut(self)
	}

	fn into_inner(self) -> (Self::Reader, Self::Writer) {
		use futures::AsyncReadExt;
		self.split()
	}
}
*/
