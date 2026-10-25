//! The "folder or extension" question of the targeted extraction.

use super::Session;
use super::input::Flow;
use crate::lang;
use p4kr_core::Filter;

impl Session<'_> {
    pub(super) fn choose_filter(&mut self) -> Flow<Filter> {
        let is_dir = loop {
            let kind = self.text(lang::filter_kind_prompt(self.lang), None)?;
            match kind.to_lowercase().as_str() {
                "d" | "f" => break true,
                "e" => break false,
                _ => writeln!(self.out, "{}", lang::answer_d_or_e(self.lang))?,
            }
        };
        let label = if is_dir {
            lang::filter_folder_label(self.lang)
        } else {
            lang::filter_extension_label(self.lang)
        };
        loop {
            let value = self.text(label, None)?;
            if value.is_empty() {
                writeln!(self.out, "{}", lang::empty_answer(self.lang))?;
                continue;
            }
            return Ok(if is_dir {
                Filter::prefix(&value)
            } else {
                Filter::ext(&value)
            });
        }
    }
}
