//! AES-128 key read from a file (`aes_key = "<32 hex>"`, as in `testdata/synth.key.toml`).
//! Never a key in the code; error messages and `Debug` never show a single byte of it.

use crate::{P4kError, Result};
use std::path::Path;

const KEY_LEN: usize = 16;

pub struct Key([u8; KEY_LEN]);

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(…)")
    }
}

/// Value of a TOML string between `"…"` or `'…'`, followed at most by a `#` comment.
fn quoted(raw: &str) -> Option<&str> {
    let raw = raw.trim();
    let quote = raw.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let (value, rest) = raw.strip_prefix(quote)?.split_once(quote)?;
    let rest = rest.trim();
    (rest.is_empty() || rest.starts_with('#')).then_some(value)
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl Key {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let shown = path.display();
        let text = std::fs::read_to_string(path)
            .map_err(|e| P4kError::Key(format!("{shown} unreadable: {e}")))?;
        let value = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .find(|(k, _)| k.trim() == "aes_key")
            .and_then(|(_, v)| quoted(v))
            .ok_or_else(|| {
                P4kError::Key(format!(
                    "{shown}: field 'aes_key' (hexadecimal string) missing"
                ))
            })?;
        if value.len() != 2 * KEY_LEN {
            return Err(P4kError::Key(format!(
                "{shown}: 'aes_key' must be {KEY_LEN} bytes ({} hexadecimal digits)",
                2 * KEY_LEN
            )));
        }
        let not_hex = || P4kError::Key(format!("{shown}: 'aes_key' is not hexadecimal"));
        if !value.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(not_hex());
        }
        let mut out = [0u8; KEY_LEN];
        let (pairs, _) = value.as_bytes().as_chunks::<2>();
        for (slot, &[hi, lo]) in out.iter_mut().zip(pairs) {
            let (Some(hi), Some(lo)) = (nibble(hi), nibble(lo)) else {
                return Err(not_hex());
            };
            *slot = (hi << 4) | lo;
        }
        Ok(Self(out))
    }

    pub(crate) fn bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}
