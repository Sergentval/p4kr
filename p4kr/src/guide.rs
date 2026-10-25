//! Guided mode: `p4kr` with no argument in a terminal, or `p4kr guide`. Three actions (DataForge to XML,
//! targeted extraction, verification) driven by questions, with settings remembered from one run to the next.
//! Any end of input or read error ends the guide cleanly.

pub mod ask;
mod date;
mod filter;
mod input;
mod job;
pub mod open;
pub mod settings;

pub use date::today_utc;

use crate::lang::{self, Lang};
use ask::{Answer, ask, clean_path, yes_no};
use input::{Flow, Stop, Watch};
use job::Job;
use p4kr_core::{Archive, Filter, Key, Verify};
use settings::Settings;
use std::cell::Cell;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

/// How the guide was started: `Bare` (no argument, often a double-click) keeps the window open at the end.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Launch {
    Bare,
    Explicit,
}

pub struct Ctx<'a> {
    pub input: &'a mut dyn BufRead,
    pub out: &'a mut dyn Write,
    /// Settings folder (`settings::config_dir`); `None` = nothing is read or saved.
    pub config: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// YYYY-MM-DD, UTC.
    pub today: String,
    /// Input and output are terminals: the guide may offer to open the output folder.
    pub interactive: bool,
    /// Opens a folder (`open::open_folder`); replaceable in tests.
    pub opener: fn(&Path) -> io::Result<()>,
    /// Language of every text the guide displays.
    pub lang: Lang,
}

struct Session<'a> {
    input: &'a mut dyn BufRead,
    out: &'a mut dyn Write,
    home: Option<&'a Path>,
    today: &'a str,
    config: Option<&'a Path>,
    settings: Settings,
    interactive: bool,
    opener: fn(&Path) -> io::Result<()>,
    lang: Lang,
}

pub fn run(ctx: &mut Ctx, launch: Launch) -> io::Result<()> {
    let failed = Cell::new(false);
    let mut watch = Watch {
        inner: &mut *ctx.input,
        failed: &failed,
    };
    let mut s = Session {
        input: &mut watch,
        out: &mut *ctx.out,
        home: ctx.home.as_deref(),
        today: &ctx.today,
        config: ctx.config.as_deref(),
        settings: Settings::default(),
        interactive: ctx.interactive,
        opener: ctx.opener,
        lang: ctx.lang,
    };
    match s.start().and_then(|()| s.menu_loop()) {
        Ok(()) | Err(Stop::Eof) => {}
        Err(Stop::Io(_)) if failed.get() => writeln!(s.out, "{}", lang::input_unreadable(s.lang))?,
        Err(Stop::Io(e)) => return Err(e),
    }
    if launch == Launch::Bare {
        writeln!(s.out, "{}", lang::press_enter_to_close(s.lang))?;
        s.out.flush()?;
        let _ = s.input.read_line(&mut String::new());
    }
    Ok(())
}

/// Existing folder that holds something, or that cannot be read (when in doubt, ask).
fn needs_confirmation(p: &Path) -> bool {
    p.is_dir()
        && match std::fs::read_dir(p) {
            Ok(mut d) => d.next().is_some(),
            Err(_) => true,
        }
}

impl Session<'_> {
    fn text(&mut self, prompt: &str, default: Option<&str>) -> Flow<String> {
        match ask(&mut *self.input, &mut *self.out, prompt, default, self.lang)? {
            Answer::Text(t) => Ok(t),
            Answer::Eof => Err(Stop::Eof),
        }
    }

    fn yes(&mut self, prompt: &str, default: bool) -> Flow<bool> {
        yes_no(&mut *self.input, &mut *self.out, prompt, default, self.lang)?.ok_or(Stop::Eof)
    }

    /// Loads the settings, forgets those whose path has vanished, then shows the header and the warnings.
    fn start(&mut self) -> Flow<()> {
        let l = self.lang;
        let mut warnings = Vec::new();
        if let Some(dir) = self.config {
            let (loaded, warning) = settings::load(dir, self.lang);
            self.settings = loaded;
            warnings.extend(warning);
        }
        for (label, slot) in [
            (lang::saved_archive_label(l), &mut self.settings.archive),
            (lang::saved_key_label(l), &mut self.settings.key_file),
            (lang::saved_folder_label(l), &mut self.settings.out_dir),
        ] {
            if let Some(p) = slot.take_if(|p| !p.exists()) {
                warnings.push(lang::not_found(l, label, &p.display().to_string()));
            }
        }
        self.header(&warnings)
    }

    fn header(&mut self, warnings: &[String]) -> Flow<()> {
        let show = |p: &Option<PathBuf>, none: &str| {
            p.as_ref()
                .map_or_else(|| none.to_string(), |p| p.display().to_string())
        };
        let l = self.lang;
        let (a, k, d) = (
            show(&self.settings.archive, lang::none_feminine(l)),
            show(&self.settings.key_file, lang::none_feminine(l)),
            show(&self.settings.out_dir, lang::none_masculine(l)),
        );
        writeln!(self.out, "p4kr {}", env!("CARGO_PKG_VERSION"))?;
        writeln!(self.out, "{}", lang::header_fields(l, &a, &k, &d))?;
        for w in warnings {
            writeln!(self.out, "{w}")?;
        }
        Ok(())
    }

    fn menu_loop(&mut self) -> Flow<()> {
        loop {
            writeln!(self.out, "{}", lang::menu_title(self.lang))?;
            let choice = self.text(lang::choice_prompt(self.lang), None)?;
            match choice.to_lowercase().as_str() {
                "1" => self.dataforge()?,
                "2" => self.targeted()?,
                "3" => self.verify()?,
                "q" => return Ok(()),
                _ => writeln!(self.out, "{}", lang::unknown_choice(self.lang, &choice))?,
            }
        }
    }

    /// Typed path, made absolute so the settings do not depend on the launch folder: the proposed default is
    /// kept as is, anything else is cleaned (quotes, `\ `, `~`).
    fn parse_path(&self, typed: &str, default: Option<&Path>) -> PathBuf {
        let path = match default {
            Some(d) if d.to_string_lossy() == typed => d.to_path_buf(),
            _ => clean_path(typed, self.home),
        };
        std::path::absolute(&path).unwrap_or(path)
    }

    fn choose_archive(&mut self) -> Flow<Archive> {
        loop {
            let default = self.settings.archive.clone();
            let shown = default.as_ref().map(|p| p.display().to_string());
            let typed = self.text(lang::archive_prompt(self.lang), shown.as_deref())?;
            let mut path = self.parse_path(&typed, default.as_deref());
            if path.is_dir() {
                let inside = path.join("Data.p4k");
                if !inside.is_file() {
                    let shown = path.display().to_string();
                    writeln!(self.out, "{}", lang::is_a_folder(self.lang, &shown))?;
                    continue;
                }
                let found = inside.display().to_string();
                writeln!(self.out, "{}", lang::archive_found(self.lang, &found))?;
                path = inside;
            }
            match Archive::open(&path) {
                Ok(a) => {
                    self.settings.archive = Some(path);
                    return Ok(a);
                }
                Err(e) => writeln!(self.out, "{}", lang::cannot_open(self.lang, &e))?,
            }
        }
    }

    fn choose_key(&mut self) -> Flow<Option<Key>> {
        loop {
            let default = self.settings.key_file.clone();
            let shown = default.as_ref().map(|p| p.display().to_string());
            let prompt = if default.is_some() {
                lang::key_prompt_with_default(self.lang)
            } else {
                lang::key_prompt_without_default(self.lang)
            };
            let typed = self.text(prompt, shown.as_deref())?;
            if typed.is_empty() || typed == "-" {
                self.settings.key_file = None;
                return Ok(None);
            }
            let path = self.parse_path(&typed, default.as_deref());
            match Key::from_file(&path) {
                Ok(k) => {
                    self.settings.key_file = Some(path);
                    return Ok(Some(k));
                }
                Err(e) => writeln!(self.out, "{}", lang::key_unreadable(self.lang, &e))?,
            }
        }
    }

    /// Output folder: proposed under the saved folder (or `<home>/p4kr`, or `p4kr`), of which it becomes a child.
    fn choose_out(&mut self, stem: &str) -> Flow<PathBuf> {
        let base = self.settings.out_dir.clone().unwrap_or_else(|| {
            let base = self
                .home
                .map_or_else(|| PathBuf::from("p4kr"), |h| h.join("p4kr"));
            std::path::absolute(&base).unwrap_or(base)
        });
        let default = base.join(format!("{stem}-{}", self.today));
        let shown = default.display().to_string();
        loop {
            let typed = self.text(lang::output_prompt(self.lang), Some(&shown))?;
            let path = self.parse_path(&typed, Some(&default));
            if path.exists() && !path.is_dir() {
                let shown = path.display().to_string();
                writeln!(self.out, "{}", lang::not_a_folder(self.lang, &shown))?;
                continue;
            }
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                self.settings.out_dir = Some(parent.to_path_buf());
            }
            return Ok(path);
        }
    }

    fn open_inputs(&mut self) -> Flow<(Archive, Option<Key>)> {
        let archive = self.choose_archive()?;
        let key = self.choose_key()?;
        Ok((archive, key))
    }

    fn pick(&mut self, archive: &Archive, found: &[usize]) -> Flow<usize> {
        for (n, &i) in found.iter().enumerate() {
            writeln!(self.out, "{}) {}", n + 1, archive.name(i).unwrap_or("?"))?;
        }
        loop {
            let typed = self.text(lang::pick_prompt(self.lang), Some("1"))?;
            match typed.parse::<usize>() {
                Ok(n) if (1..=found.len()).contains(&n) => return Ok(found[n - 1]),
                _ => writeln!(self.out, "{}", lang::pick_range(self.lang, found.len()))?,
            }
        }
    }

    fn dataforge(&mut self) -> Flow<()> {
        let (archive, key) = self.open_inputs()?;
        let found = archive.select(&Filter::ext(".dcb"));
        let indices = match found.len() {
            0 => {
                writeln!(self.out, "{}", lang::no_dataforge(self.lang))?;
                return Ok(());
            }
            1 => found,
            _ => vec![self.pick(&archive, &found)?],
        };
        let out = Some(self.choose_out("dataforge")?);
        let job = Job {
            archive,
            key,
            indices,
            out,
            verify: Verify::Crc,
            xml: true,
            action: lang::action_dataforge(self.lang),
        };
        self.execute(&job)
    }

    fn targeted(&mut self) -> Flow<()> {
        let (archive, key) = self.open_inputs()?;
        let filter = self.choose_filter()?;
        let indices = archive.select(&filter);
        if indices.is_empty() {
            writeln!(self.out, "{}", lang::no_match(self.lang))?;
            return Ok(());
        }
        let xml = self.yes(lang::convert_xml_question(self.lang), true)?;
        let out = Some(self.choose_out("extraction")?);
        let job = Job {
            archive,
            key,
            indices,
            out,
            verify: Verify::Crc,
            xml,
            action: lang::action_targeted(self.lang),
        };
        self.execute(&job)
    }

    fn verify(&mut self) -> Flow<()> {
        let (archive, key) = self.open_inputs()?;
        let indices = archive.select(&Filter::All);
        if indices.is_empty() {
            writeln!(self.out, "{}", lang::no_match(self.lang))?;
            return Ok(());
        }
        let job = Job {
            archive,
            key,
            indices,
            out: None,
            verify: Verify::Full,
            xml: false,
            action: lang::action_verify(self.lang),
        };
        self.execute(&job)
    }

    /// Summary, confirmations, execution, then settings saved. A refusal goes back to the menu.
    fn execute(&mut self, job: &Job) -> Flow<()> {
        let l = self.lang;
        writeln!(self.out, "{}", lang::action_line(l, job.action))?;
        let path = job.archive.path().display().to_string();
        writeln!(self.out, "{}", lang::archive_line(l, &path))?;
        let counts = lang::job_counts(
            l,
            job.indices.len(),
            job.stored_bytes(),
            &job.destination(l),
        );
        writeln!(self.out, "{counts}")?;
        if job.out.as_deref().is_some_and(needs_confirmation)
            && !self.yes(lang::folder_not_empty(l), false)?
        {
            return Ok(());
        }
        if !self.yes(lang::launch_prompt(l), true)? {
            return Ok(());
        }
        let written = job.run(&mut *self.out, l)?;
        self.remember()?;
        match written {
            Some(dir) if self.interactive => self.offer_open(&dir),
            _ => Ok(()),
        }
    }

    /// Offers to open the output folder; end of input counts as "no".
    fn offer_open(&mut self, dir: &Path) -> Flow<()> {
        if !dir.is_dir() {
            return Ok(());
        }
        if self.yes(lang::open_folder_prompt(self.lang), true)?
            && let Err(e) = (self.opener)(dir)
        {
            writeln!(self.out, "{}", lang::cannot_open_folder(self.lang, &e))?;
        }
        Ok(())
    }

    /// Saves the settings; a failure is only a warning.
    fn remember(&mut self) -> io::Result<()> {
        let Some(dir) = self.config else {
            return Ok(());
        };
        match settings::save(dir, &self.settings, self.lang) {
            Ok(()) => Ok(()),
            Err(e) => writeln!(self.out, "{}", lang::settings_not_saved(self.lang, &e)),
        }
    }
}

#[cfg(test)]
mod tests;
