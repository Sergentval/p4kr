//! List of outputs (one file per record path), safe paths and rendering of an output.

use crate::record::{self, Halt};
use crate::{Db, RecordError};
use p4kr_xmlwrite::{Profile, WriteError, write};
use std::collections::HashMap;
use std::path::Path;

/// An output: relative path (text of the record) and the record that fills it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub path: String,
    pub record: u32,
}

/// Marker of a .NET call stack written in an attribute (field error text), on both sides.
const TRACE_MARK: &[u8] = b"&#xA;   at ";

impl Db<'_> {
    /// Order of the reference's save: records in order, a path already seen keeps its
    /// place and takes the most recent record.
    pub fn outputs(&self) -> Vec<Output> {
        let mut list: Vec<Output> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for record in 0..self.record_count() as u32 {
            // Invariant: `open` checked every path (`RecordPath`), so reading cannot fail;
            // in a production build, an unreadable record would simply be skipped.
            let path = self.record_def(record).and_then(|r| self.text(r.path).ok());
            debug_assert!(path.is_some(), "path of record {record} unreadable");
            let Some(path) = path else {
                continue;
            };
            match seen.get(&path) {
                Some(&k) => list[k].record = record,
                None => {
                    seen.insert(path.clone(), list.len());
                    list.push(Output { path, record });
                }
            }
        }
        list
    }

    /// XML of the output, `None` when the reference writes no file.
    pub fn render(&self, o: &Output) -> Result<Option<Vec<u8>>, RecordError> {
        if safe_relative(&o.path).is_none() {
            return Err(RecordError::UnsafePath);
        }
        let doc = record::record_doc(self, o.record).map_err(|h| match h {
            Halt::Size => RecordError::TooLarge,
            Halt::Work => RecordError::TooMuchWork,
        })?;
        let Some(doc) = doc else {
            return Ok(None);
        };
        let mut out = Vec::new();
        write(&doc, Profile::DataForge, &mut out).map_err(|e| match e {
            WriteError::TooLarge => RecordError::TooLarge,
            WriteError::Name(_) => RecordError::BadName,
            WriteError::NoRoot | WriteError::CDataEnd => RecordError::Write,
        })?;
        Ok(Some(out))
    }
}

/// Relative path under the output folder: `None` if it is empty, absolute, or has a `..` component.
/// Only `/` separates: `\` is a file-name character, as for the reference on Linux.
pub fn safe_relative(path: &str) -> Option<&Path> {
    if path.is_empty() || path.starts_with('/') || path.split('/').any(|c| c == "..") {
        return None;
    }
    Some(Path::new(path))
}

/// The output contains a field error text with its call stack (not reproducible: the reference then
/// compares only the presence of the marker).
pub fn has_trace(xml: &[u8]) -> bool {
    xml.windows(TRACE_MARK.len()).any(|w| w == TRACE_MARK)
}
