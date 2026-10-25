//! An action ready to run (extraction, DataForge conversion or verification) and its execution.

use crate::lang::{self, Lang};
use crate::progress::Progress;
use crate::report::extract_summary;
use p4kr_core::extract::{Extras, Options, extract_with};
use p4kr_core::{Archive, Key, Verify};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

/// What the user chose to run.
pub struct Job {
    pub archive: Archive,
    pub key: Option<Key>,
    pub indices: Vec<usize>,
    /// `None` = verification only.
    pub out: Option<PathBuf>,
    pub verify: Verify,
    pub xml: bool,
    /// Label shown in the summary before the run.
    pub action: &'static str,
}

impl Job {
    pub fn stored_bytes(&self) -> u64 {
        self.indices
            .iter()
            .filter_map(|&i| self.archive.entry(i))
            .map(|e| e.csize)
            .sum()
    }

    pub fn destination(&self, lang: Lang) -> String {
        self.out.as_ref().map_or_else(
            || lang::verify_only(lang).to_string(),
            |o| o.display().to_string(),
        )
    }

    /// Runs, then writes problems, summary and output folder to `out`. Returns the absolute output folder
    /// if the extraction succeeded and wrote somewhere.
    pub fn run(&self, out: &mut dyn Write, lang: Lang) -> io::Result<Option<PathBuf>> {
        let opts = Options {
            out: self.out.as_deref(),
            key: self.key.as_ref(),
            verify: self.verify,
            threads: 0,
            fail_fast: false,
        };
        let bar = Progress::start(self.indices.len(), false, lang);
        let extras = Extras {
            xml: self.xml,
            progress: Some(bar.counter()),
            records: Some(bar.records()),
        };
        let started = Instant::now();
        let result = extract_with(&self.archive, &self.indices, &opts, &extras);
        bar.finish();
        let report = match result {
            Ok(r) => r,
            Err(e) => {
                writeln!(out, "{}", lang::extraction_refused(lang, &e))?;
                return Ok(None);
            }
        };
        writeln!(
            out,
            "{}",
            lang::done_in(lang, started.elapsed().as_secs_f64())
        )?;
        let summary = extract_summary(&self.archive, &report, self.xml, lang);
        for p in summary.problems.iter().chain(&summary.notices) {
            writeln!(out, "{p}")?;
        }
        writeln!(out, "{}", summary.line)?;
        let abs = self
            .out
            .as_ref()
            .map(|o| std::path::absolute(o).unwrap_or_else(|_| o.clone()));
        if let Some(a) = &abs {
            writeln!(out, "{}", lang::output_line(lang, &a.display().to_string()))?;
        }
        Ok(abs)
    }
}
