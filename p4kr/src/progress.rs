//! Progress bar on the error stream, only when it is a terminal (feedback from unp4k users).

use crate::lang::{self, Lang};
use p4kr_core::extract::RecordProgress;
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::thread::JoinHandle;
use std::time::Duration;

const WIDTH: usize = 30;
const TICK: Duration = Duration::from_millis(200);

/// Line shown: bar, percentage, count and unit (entries, records).
pub fn line(done: usize, total: usize, unit: &str) -> String {
    let done = done.min(total);
    let (filled, pct) = match (done * WIDTH).checked_div(total) {
        Some(filled) => (filled, done * 100 / total),
        None => (WIDTH, 100),
    };
    format!(
        "[{}{}] {pct:>3} % {done}/{total} {unit}",
        "#".repeat(filled),
        "-".repeat(WIDTH - filled)
    )
}

/// The records bar as soon as the DataForge phase has announced its total, the entries bar otherwise.
fn current(
    entries: &AtomicUsize,
    total: usize,
    unit: &str,
    records: &RecordProgress,
    lang: Lang,
) -> String {
    match records.total.load(Relaxed) {
        0 => line(entries.load(Relaxed), total, unit),
        n => line(records.done.load(Relaxed), n, lang::unit_records(lang)),
    }
}

pub struct Progress {
    done: Arc<AtomicUsize>,
    records: Arc<RecordProgress>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Progress {
    pub fn start(total: usize, quiet: bool, lang: Lang) -> Self {
        Self::with_unit(total, quiet, lang::unit_entries(lang), lang)
    }

    /// Bar whose `counter()` counts records (`dcb` command).
    pub fn start_records(total: usize, quiet: bool, lang: Lang) -> Self {
        Self::with_unit(total, quiet, lang::unit_records(lang), lang)
    }

    fn with_unit(total: usize, quiet: bool, unit: &'static str, lang: Lang) -> Self {
        let done = Arc::new(AtomicUsize::new(0));
        let records = Arc::new(RecordProgress::default());
        let stop = Arc::new(AtomicBool::new(false));
        let thread = (!quiet && std::io::stderr().is_terminal()).then(|| {
            let (done, stop) = (Arc::clone(&done), Arc::clone(&stop));
            let records = Arc::clone(&records);
            std::thread::spawn(move || {
                let mut err = std::io::stderr();
                while !stop.load(Relaxed) {
                    let _ = write!(err, "\r{}", current(&done, total, unit, &records, lang));
                    let _ = err.flush();
                    std::thread::sleep(TICK);
                }
                let _ = write!(err, "\r{}\r", " ".repeat(WIDTH + 40));
                let _ = err.flush();
            })
        });
        Self {
            done,
            records,
            stop,
            thread,
        }
    }

    pub fn counter(&self) -> &AtomicUsize {
        &self.done
    }

    /// Counters of the DataForge phase: the bar switches to records as soon as their total is known.
    pub fn records(&self) -> &RecordProgress {
        &self.records
    }

    pub fn finish(mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_fills_with_the_count() {
        assert_eq!(
            line(0, 10, "entries"),
            format!("[{}]   0 % 0/10 entries", "-".repeat(30))
        );
        assert_eq!(
            line(5, 10, "entries"),
            format!("[{}{}]  50 % 5/10 entries", "#".repeat(15), "-".repeat(15))
        );
        assert_eq!(
            line(12, 10, "entries"),
            format!("[{}] 100 % 10/10 entries", "#".repeat(30))
        );
        assert_eq!(
            line(0, 0, "entries"),
            format!("[{}] 100 % 0/0 entries", "#".repeat(30))
        );
    }

    #[test]
    fn the_unit_follows_what_the_counter_counts() {
        let n = AtomicUsize::new(3);
        let records = RecordProgress::default();
        assert!(current(&n, 4, "records", &records, Lang::En).ends_with("3/4 records"));
    }

    #[test]
    fn the_records_unit_follows_the_language() {
        let entries = AtomicUsize::new(1);
        let records = RecordProgress::default();
        records.total.store(2, Relaxed);
        let l = current(&entries, 1, "entries", &records, Lang::Fr);
        assert!(
            l.ends_with(&format!("0/2 {}", lang::unit_records(Lang::Fr))),
            "{l}"
        );
    }

    #[test]
    fn the_bar_switches_to_records_once_their_total_is_known() {
        let entries = AtomicUsize::new(1);
        let records = RecordProgress::default();
        assert!(current(&entries, 1, "entries", &records, Lang::En).ends_with("1/1 entries"));
        records.total.store(62_620, Relaxed);
        records.done.store(26_300, Relaxed);
        let l = current(&entries, 1, "entries", &records, Lang::En);
        assert!(l.contains(" 41 % 26300/62620 records"), "{l}");
    }
}
