//! p4kr-core: Data.p4k reader (ZIP64 + CIG extra blocks), written after the reference
//! implementation. No key in this code: it comes from a file.

mod central;
pub mod dcb;
pub mod extract;
mod key;
mod read;
mod select;

pub use central::{Archive, EntryRef, Tail};
pub use key::Key;
pub use select::{Filter, target};

/// Typed errors; the first eight mirror the reference implementation's error classes.
/// Messages never contain an entry name or a key byte.
#[derive(Debug, thiserror::Error)]
pub enum P4kError {
    #[error("local header: {0}")]
    LocalHeaderMismatch(String),
    #[error("size: {0}")]
    SizeMismatch(String),
    #[error("CRC32C {found:#010x} ≠ {expected:#010x}")]
    CrcMismatch { found: u32, expected: u32 },
    #[error("sha256 of the stored bytes ≠ block 0x5003")]
    ShaMismatch,
    #[error("unknown or malformed extra block(s)")]
    UnknownExtra,
    #[error("decryption: {0}")]
    DecryptError(String),
    #[error("decompression: {0}")]
    DecompressError(String),
    #[error("unknown method {0}")]
    UnsupportedMethod(u16),
    #[error("unreadable archive: {0}")]
    Format(String),
    #[error("entry name outside the output folder (index {0})")]
    Refused(usize),
    #[error("key: {0}")]
    Key(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
}

impl P4kError {
    /// Error kind as reported by the reference implementation.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::LocalHeaderMismatch(_) => "local-header",
            Self::SizeMismatch(_) => "size",
            Self::CrcMismatch { .. } => "crc",
            Self::ShaMismatch => "sha5003",
            Self::UnknownExtra => "unknown-extra",
            Self::DecryptError(_) => "decrypt",
            Self::DecompressError(_) => "zstd",
            Self::UnsupportedMethod(_) => "method",
            Self::Format(_) => "exception:Format",
            Self::Refused(_) => "exception:Refused",
            Self::Key(_) => "exception:Key",
            Self::Io(_) => "exception:Io",
        }
    }
}

pub type Result<T> = std::result::Result<T, P4kError>;

/// Read-time checks: CRC32C always; `Full` adds the sha256 of the stored bytes (block 0x5003).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verify {
    #[default]
    Crc,
    Full,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_match_the_reference_error_kinds() {
        let io = std::io::Error::other("x");
        let cases: Vec<(P4kError, &str)> = vec![
            (P4kError::LocalHeaderMismatch("x".into()), "local-header"),
            (P4kError::SizeMismatch("x".into()), "size"),
            (
                P4kError::CrcMismatch {
                    found: 1,
                    expected: 2,
                },
                "crc",
            ),
            (P4kError::ShaMismatch, "sha5003"),
            (P4kError::UnknownExtra, "unknown-extra"),
            (P4kError::DecryptError("x".into()), "decrypt"),
            (P4kError::DecompressError("x".into()), "zstd"),
            (P4kError::UnsupportedMethod(7), "method"),
            (P4kError::Format("x".into()), "exception:Format"),
            (P4kError::Refused(3), "exception:Refused"),
            (P4kError::Key("x".into()), "exception:Key"),
            (P4kError::Io(io), "exception:Io"),
        ];
        for (err, kind) in cases {
            assert_eq!(err.kind(), kind, "{err:?}");
        }
    }

    #[test]
    fn verify_defaults_to_crc_only() {
        assert_eq!(Verify::default(), Verify::Crc);
    }
}
