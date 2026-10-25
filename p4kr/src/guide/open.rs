//! Opens a folder in the system file manager, without waiting and without a dependency.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

#[cfg(windows)]
const OPENER: &str = "explorer";
#[cfg(target_os = "macos")]
const OPENER: &str = "open";
#[cfg(not(any(windows, target_os = "macos")))]
const OPENER: &str = "xdg-open";

/// Launches the file manager on `dir` then returns (silenced output).
pub fn open_folder(dir: &Path) -> io::Result<()> {
    Command::new(OPENER)
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}
