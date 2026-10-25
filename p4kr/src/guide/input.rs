//! Reading the guide's input: end of the guide (`Stop`) and detection of read errors (`Watch`).

use std::cell::Cell;
use std::io::{self, BufRead, Read};

/// End of the guide: input exhausted, or an I/O error.
pub enum Stop {
    Eof,
    Io(io::Error),
}

impl From<io::Error> for Stop {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Flow<T> = Result<T, Stop>;

/// Input that notes any read error (invalid UTF-8 included), to tell it apart from a write error.
pub struct Watch<'a> {
    pub inner: &'a mut dyn BufRead,
    pub failed: &'a Cell<bool>,
}

impl Watch<'_> {
    fn note<T>(&self, r: io::Result<T>) -> io::Result<T> {
        if r.is_err() {
            self.failed.set(true);
        }
        r
    }
}

impl Read for Watch<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let r = self.inner.read(buf);
        self.note(r)
    }
}

impl BufRead for Watch<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let r = self.inner.fill_buf();
        if r.is_err() {
            self.failed.set(true);
        }
        r
    }
    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt);
    }
    fn read_line(&mut self, buf: &mut String) -> io::Result<usize> {
        let r = self.inner.read_line(buf);
        self.note(r)
    }
}
