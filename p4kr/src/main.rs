//! p4kr: list, extract and verify the entries of a Data.p4k archive.

use clap::{Args, CommandFactory, Parser, Subcommand};
use p4kr_core::dcb::write_outputs;
use p4kr_core::extract::{Extras, Options, extract_with};
use p4kr_core::{Archive, Filter, Key, Verify};
use p4kr_cryxml::{Outcome, convert};
use p4kr_dataforge::Db;
use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use report::{Summary, dcb_summary, extract_summary};

mod guide;
mod lang;
mod progress;
mod report;

/// The subcommands always speak English; only the guided mode follows the system language.
const CLI_LANG: lang::Lang = lang::Lang::En;

#[derive(Parser)]
#[command(
    name = "p4kr",
    version,
    about = "Data.p4k reader: list, extract, verify"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Guided mode, step by step
    Guide,
    /// List the names of the selected entries
    List {
        archive: PathBuf,
        #[command(flatten)]
        sel: Selection,
    },
    /// Extract the selected entries (CRC32C always verified)
    Extract {
        archive: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[command(flatten)]
        read: ReadOpts,
        #[command(flatten)]
        sel: Selection,
        /// Also verify the sha256 of the stored bytes (block 0x5003)
        #[arg(long)]
        verify: bool,
        /// Stop at the first entry in error
        #[arg(long)]
        fail_fast: bool,
        /// Convert binary XML to text XML, like unp4k + unforge
        #[arg(long)]
        xml: bool,
        /// No progress bar
        #[arg(long)]
        quiet: bool,
    },
    /// Read and verify (CRC32C and sha256) the selected entries, writing nothing
    Verify {
        archive: PathBuf,
        #[command(flatten)]
        read: ReadOpts,
        #[command(flatten)]
        sel: Selection,
        /// No progress bar
        #[arg(long)]
        quiet: bool,
    },
    /// Convert a binary XML file (CryXmlB) to text XML, like unforge
    Xml { input: PathBuf, output: PathBuf },
    /// Write the records of a DataForge (.dcb) file as XML into a folder
    Dcb {
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Number of threads (0 = all cores)
        #[arg(long, default_value_t = 0)]
        threads: usize,
        /// No progress bar
        #[arg(long)]
        quiet: bool,
    },
    /// Write one entry to standard output (converted with --xml), no file
    Cat {
        archive: PathBuf,
        #[arg(long)]
        exact: String,
        #[arg(long)]
        xml: bool,
        #[arg(long)]
        key_file: Option<PathBuf>,
    },
}

/// Each option can be repeated (OR between its values); `--prefix` and `--ext` together: both (AND).
#[derive(Args)]
struct Selection {
    /// Exact path (`/` or `\`), repeatable
    #[arg(long, conflicts_with_all = ["prefix", "ext"])]
    exact: Vec<String>,
    /// Folder (every entry below it), repeatable; with --ext: both at once
    #[arg(long)]
    prefix: Vec<String>,
    /// Extension, case-insensitive (e.g. `.xml`), repeatable
    #[arg(long)]
    ext: Vec<String>,
}

#[derive(Args)]
struct ReadOpts {
    /// Key file (`aes_key = "<32 hex>"`)
    #[arg(long)]
    key_file: Option<PathBuf>,
    /// Number of threads (0 = all cores)
    #[arg(long, default_value_t = 0)]
    threads: usize,
}

impl Selection {
    fn filter(&self) -> Filter {
        let any = |values: &[String], one: fn(&str) -> Filter| {
            Filter::any(values.iter().map(|v| one(v)).collect())
        };
        Filter::every(vec![
            any(&self.exact, Filter::exact),
            any(&self.prefix, Filter::prefix),
            any(&self.ext, Filter::ext),
        ])
    }
}

fn open(path: &Path) -> Result<Archive, u8> {
    Archive::open(path).map_err(|e| {
        eprintln!("cannot open: {e}");
        3
    })
}

fn load_key(path: Option<&Path>) -> Result<Option<Key>, u8> {
    path.map(Key::from_file).transpose().map_err(|e| {
        eprintln!("configuration error: {e}");
        2
    })
}

/// Prints problems and warnings on standard error, then the summary on standard output;
/// returns the exit code.
fn emit(s: &Summary) -> u8 {
    for p in s.problems.iter().chain(&s.notices) {
        eprintln!("{p}");
    }
    println!("{}", s.line);
    s.code
}

/// Settings of a read, apart from the selection and the archive.
#[derive(Clone, Copy)]
struct Mode {
    verify: Verify,
    fail_fast: bool,
    xml: bool,
    quiet: bool,
}

fn run_read(
    archive: &Path,
    read: &ReadOpts,
    sel: &Selection,
    out: Option<&Path>,
    mode: Mode,
) -> Result<u8, u8> {
    let key = load_key(read.key_file.as_deref())?;
    let a = open(archive)?;
    let indices = a.select(&sel.filter());
    let opts = Options {
        out,
        key: key.as_ref(),
        verify: mode.verify,
        threads: read.threads,
        fail_fast: mode.fail_fast,
    };
    let bar = progress::Progress::start(indices.len(), mode.quiet, CLI_LANG);
    let extras = Extras {
        xml: mode.xml,
        progress: Some(bar.counter()),
        records: Some(bar.records()),
    };
    let result = extract_with(&a, &indices, &opts, &extras);
    bar.finish();
    match result {
        Ok(r) => Ok(emit(&extract_summary(&a, &r, mode.xml, CLI_LANG))),
        Err(e) => {
            eprintln!("extraction refused: {e}");
            Err(1)
        }
    }
}

fn run_dcb(input: &Path, out: &Path, threads: usize, quiet: bool) -> Result<u8, u8> {
    let data = std::fs::read(input).map_err(|e| {
        eprintln!("cannot read: {e}");
        1
    })?;
    let db = Db::open(&data).map_err(|e| {
        eprintln!("DataForge file refused: {e}");
        1
    })?;
    let outputs = db.outputs();
    let bar = progress::Progress::start_records(outputs.len(), quiet, CLI_LANG);
    let result = write_outputs(
        &db,
        &outputs,
        out,
        &HashSet::new(),
        threads,
        Some(bar.counter()),
    );
    bar.finish();
    let d = result.map_err(|e| {
        eprintln!("write refused: {e}");
        1
    })?;
    Ok(emit(&dcb_summary(&d, CLI_LANG)))
}

fn run_xml(input: &Path, output: &Path) -> Result<u8, u8> {
    let data = std::fs::read(input).map_err(|e| {
        eprintln!("cannot read: {e}");
        1
    })?;
    let c = convert(&data);
    if let Some(xml) = &c.xml {
        std::fs::write(output, xml).map_err(|e| {
            eprintln!("cannot write: {e}");
            1
        })?;
    }
    match (c.outcome, &c.error) {
        (Outcome::Converted, _) => println!("converted"),
        (Outcome::Text, _) => println!("already XML"),
        (Outcome::Other, _) => println!("not a CryXmlB"),
        (Outcome::Failed, e) => {
            println!(
                "failed: {}",
                e.as_ref().map_or_else(String::new, ToString::to_string)
            );
            return Ok(1);
        }
    }
    Ok(0)
}

fn run_cat(archive: &Path, exact: &str, xml: bool, key_file: Option<&Path>) -> Result<u8, u8> {
    let key = load_key(key_file)?;
    let a = open(archive)?;
    let [i] = a.select(&Filter::exact(exact))[..] else {
        eprintln!("entry not found or ambiguous");
        return Err(1);
    };
    let data = a.read(i, key.as_ref(), Verify::Crc).map_err(|e| {
        eprintln!("read: {}", e.kind());
        1
    })?;
    let c = xml.then(|| convert(&data));
    let bytes = c.as_ref().and_then(|c| c.xml.as_deref()).unwrap_or(&data);
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .map_err(|_| 1u8)?;
    Ok(u8::from(c.is_some_and(|c| c.outcome == Outcome::Failed)))
}

fn run_list(archive: &Path, sel: &Selection) -> Result<u8, u8> {
    let a = open(archive)?;
    for i in a.select(&sel.filter()) {
        println!("{}", a.name(i).unwrap_or("?"));
    }
    Ok(0)
}

fn run_guide(launch: guide::Launch) -> Result<u8, u8> {
    let home = guide::ask::home_dir();
    let lang = lang::current();
    let mut ctx = guide::Ctx {
        input: &mut std::io::stdin().lock(),
        out: &mut std::io::stdout().lock(),
        config: guide::settings::config_dir(home.as_deref()),
        home,
        today: guide::today_utc(),
        interactive: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        opener: guide::open::open_folder,
        lang,
    };
    guide::run(&mut ctx, launch).map_err(|e| {
        eprintln!("{}", lang::guide_interrupted(lang, &e));
        1
    })?;
    Ok(0)
}

/// Without a subcommand: the guide in a terminal, the help otherwise (exit code 2).
fn run_bare() -> Result<u8, u8> {
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return run_guide(guide::Launch::Bare);
    }
    eprint!("{}", Cli::command().render_help());
    Err(2)
}

fn run(cmd: Option<Cmd>) -> Result<u8, u8> {
    let Some(cmd) = cmd else {
        return run_bare();
    };
    match cmd {
        Cmd::Guide => run_guide(guide::Launch::Explicit),
        Cmd::List { archive, sel } => run_list(&archive, &sel),
        Cmd::Extract {
            archive,
            out,
            read,
            sel,
            verify,
            fail_fast,
            xml,
            quiet,
        } => {
            let mode = Mode {
                verify: if verify { Verify::Full } else { Verify::Crc },
                fail_fast,
                xml,
                quiet,
            };
            run_read(&archive, &read, &sel, Some(&out), mode)
        }
        Cmd::Verify {
            archive,
            read,
            sel,
            quiet,
        } => {
            let mode = Mode {
                verify: Verify::Full,
                fail_fast: false,
                xml: false,
                quiet,
            };
            run_read(&archive, &read, &sel, None, mode)
        }
        Cmd::Dcb {
            input,
            out,
            threads,
            quiet,
        } => run_dcb(&input, &out, threads, quiet),
        Cmd::Xml { input, output } => run_xml(&input, &output),
        Cmd::Cat {
            archive,
            exact,
            xml,
            key_file,
        } => run_cat(&archive, &exact, xml, key_file.as_deref()),
    }
}

fn main() -> ExitCode {
    let code = run(Cli::parse().cmd).unwrap_or_else(|c| c);
    ExitCode::from(code)
}
