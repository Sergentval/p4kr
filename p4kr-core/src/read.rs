//! Reading an entry: order and checks of the reference implementation.

use crate::central::{BAD_EXTRA, ENCRYPTED, HAS_SHA, Row, u16_at};
use crate::{Archive, Key, P4kError, Result, Verify};
use aes::Aes128;
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::fs::File;
use std::ops::Range;
use zstd::stream::raw::{Decoder, InBuffer, Operation, OutBuffer};

/// Standard `PK\x03\x04`; `PK\x03\x14` = CIG variant (4.10 archives), same layout.
const LOCAL_SIGS: [&[u8]; 2] = [b"PK\x03\x04", b"PK\x03\x14"];
const SIG_LEN: usize = 4;
const LOCAL_LEN: usize = 30;
const STORED: u16 = 0;
const ZSTD: u16 = 100;
const AES_BLOCK: usize = 16;
/// Smallest growth step of the zstd output, and size of the buffer that swallows the excess.
const MIN_CHUNK: usize = 64 << 10;
const MAX_CHUNK: usize = 64 << 20;

impl Archive {
    pub fn read(&self, i: usize, key: Option<&Key>, verify: Verify) -> Result<Vec<u8>> {
        let row = self.row(i)?;
        let span = self.data_span(row)?;
        let stored = self.bytes().get(span).ok_or_else(truncated)?;
        decode(row, Cow::Borrowed(stored), self.sha_of(row), key, verify)
    }

    pub fn read_at(
        &self,
        file: &File,
        i: usize,
        key: Option<&Key>,
        verify: Verify,
    ) -> Result<Vec<u8>> {
        let row = self.row(i)?;
        let span = self.data_span(row)?;
        let mut buf = vec![0u8; span.len()]; // bounded by the file size (data_span)
        read_exact_at(file, &mut buf, span.start as u64)?;
        decode(row, Cow::Owned(buf), self.sha_of(row), key, verify)
    }

    fn sha_of(&self, row: &Row) -> Option<&[u8]> {
        if row.flags & HAS_SHA == 0 {
            return None;
        }
        let at = usize::try_from(row.sha_at).ok()?;
        self.bytes().get(at..at.checked_add(32)?)
    }

    /// Local header: method @8, name length @26, extra length @28; data right after.
    /// Returns the range of the stored bytes, always inside the file.
    fn data_span(&self, row: &Row) -> Result<Range<usize>> {
        if row.flags & BAD_EXTRA != 0 {
            return Err(P4kError::UnknownExtra);
        }
        let b = self.bytes();
        let mismatch = |m: String| P4kError::LocalHeaderMismatch(m);
        let off = usize::try_from(row.local_offset)
            .ok()
            .filter(|&o| {
                o.checked_add(LOCAL_LEN).is_some_and(|e| e <= b.len())
                    && b.get(o..o + SIG_LEN)
                        .is_some_and(|s| LOCAL_SIGS.contains(&s))
            })
            .ok_or_else(|| mismatch(format!("no local header at {:#x}", row.local_offset)))?;
        let (Some(method), Some(nlen), Some(elen)) =
            (u16_at(b, off + 8), u16_at(b, off + 26), u16_at(b, off + 28))
        else {
            return Err(mismatch(format!("local header truncated at {off:#x}")));
        };
        if method != row.method {
            return Err(mismatch(format!("local method {method} ≠ {}", row.method)));
        }
        let (nlen, elen) = (usize::from(nlen), usize::from(elen));
        let raw = usize::try_from(row.raw_name_at)
            .ok()
            .and_then(|at| b.get(at..at.checked_add(usize::from(row.raw_name_len))?));
        if raw.is_none() || b.get(off + LOCAL_LEN..off + LOCAL_LEN + nlen) != raw {
            return Err(mismatch("local header name differs".into()));
        }
        let start = off + LOCAL_LEN + nlen + elen;
        let end = (start as u64)
            .checked_add(row.csize)
            .filter(|&e| e <= b.len() as u64)
            .ok_or_else(|| {
                P4kError::SizeMismatch(format!(
                    "{} bytes announced past the end of the file (truncated archive?)",
                    row.csize
                ))
            })?;
        Ok(start..end as usize)
    }
}

fn truncated() -> P4kError {
    P4kError::SizeMismatch("data outside the file (truncated archive?)".into())
}

fn decode(
    row: &Row,
    stored: Cow<'_, [u8]>,
    sha: Option<&[u8]>,
    key: Option<&Key>,
    verify: Verify,
) -> Result<Vec<u8>> {
    if verify == Verify::Full
        && let Some(expected) = sha
        && Sha256::digest(&stored).as_slice() != expected
    {
        return Err(P4kError::ShaMismatch);
    }
    let encrypted = row.flags & ENCRYPTED != 0;
    let plain = if encrypted {
        Cow::Owned(decrypt(stored.into_owned(), key)?)
    } else {
        stored
    };
    let data = match row.method {
        STORED => plain.into_owned(),
        ZSTD => unzstd(&plain, row.size, encrypted)?,
        m => return Err(P4kError::UnsupportedMethod(m)),
    };
    if data.len() as u64 != row.size {
        return Err(P4kError::SizeMismatch(format!(
            "{} bytes decompressed, {} expected",
            data.len(),
            row.size
        )));
    }
    let found = crc32c::crc32c(&data);
    if found != row.crc {
        return Err(P4kError::CrcMismatch {
            found,
            expected: row.crc,
        });
    }
    Ok(data)
}

fn decrypt(mut buf: Vec<u8>, key: Option<&Key>) -> Result<Vec<u8>> {
    let key =
        key.ok_or_else(|| P4kError::DecryptError("encrypted entry and no key provided".into()))?;
    if !buf.len().is_multiple_of(AES_BLOCK) {
        return Err(P4kError::DecryptError(format!(
            "{} bytes, not a multiple of {AES_BLOCK}",
            buf.len()
        )));
    }
    cbc::Decryptor::<Aes128>::new(key.bytes().into(), &[0u8; AES_BLOCK].into())
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|_| P4kError::DecryptError("invalid AES block".into()))?;
    Ok(buf)
}

/// A single frame. The output grows with the data actually produced and never exceeds
/// `size + 1` bytes; beyond that, the decoder keeps going into a discarded buffer until the end of the frame
/// so that the "incomplete frame" and "bytes after the frame" rules take precedence over `SizeMismatch`
/// (order of the reference implementation). An output of `size + 1` bytes is returned as is to the caller.
fn unzstd(src: &[u8], size: u64, encrypted: bool) -> Result<Vec<u8>> {
    let err = |m: String| P4kError::DecompressError(m);
    let limit = usize::try_from(size)
        .unwrap_or(usize::MAX)
        .saturating_add(1);
    let mut dec = Decoder::new().map_err(|e| err(e.to_string()))?;
    let mut input = InBuffer::around(src);
    let mut out: Vec<u8> = Vec::new(); // `out[..filled]` is the output; the rest is zeroed room
    let mut filled = 0usize;
    let mut discard: Vec<u8> = Vec::new();
    loop {
        let before = input.pos();
        let (hint, produced) = if filled < limit {
            if filled == out.len() {
                let step = out.len().clamp(MIN_CHUNK, MAX_CHUNK).min(limit - filled);
                out.resize(filled + step, 0);
            }
            let room = out
                .get_mut(filled..)
                .ok_or_else(|| err("output buffer".into()))?;
            let mut ob = OutBuffer::around(room);
            let hint = dec
                .run(&mut input, &mut ob)
                .map_err(|e| err(e.to_string()))?;
            let produced = ob.pos();
            filled += produced;
            (hint, produced)
        } else {
            if discard.is_empty() {
                discard.resize(MIN_CHUNK, 0);
            }
            let mut ob = OutBuffer::around(&mut discard[..]);
            let hint = dec
                .run(&mut input, &mut ob)
                .map_err(|e| err(e.to_string()))?;
            (hint, ob.pos())
        };
        if hint == 0 {
            let rest = src.get(input.pos()..).unwrap_or_default();
            if !rest.is_empty() && (!encrypted || rest.iter().any(|&b| b != 0)) {
                let n = rest.len();
                let unit = if n == 1 { "byte" } else { "bytes" };
                return Err(err(format!("{n} {unit} after the zstd frame")));
            }
            out.truncate(filled);
            return Ok(out);
        }
        if produced == 0 && input.pos() == before {
            return Err(err("zstd frame incomplete".into()));
        }
    }
}

/// Reads exactly `buf.len()` bytes at `offset`, at the explicit offset (parallel read).
/// On Windows, `seek_read` moves the handle's cursor; the code never relies on it, the offset is explicit
/// and each thread has its own `File`.
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            match file.seek_read(&mut buf[done..], offset + done as u64) {
                Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}
