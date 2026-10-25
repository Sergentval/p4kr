//! User-facing text in English and French. Every French string of the program lives in this file.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Fr,
}

/// Primary language id of French in a Windows LANGID.
const LANG_FRENCH: u16 = 0x0c;

impl Lang {
    pub fn pick(self, en: &'static str, fr: &'static str) -> &'static str {
        match self {
            Lang::En => en,
            Lang::Fr => fr,
        }
    }
}

fn parse(value: &str) -> Option<Lang> {
    match value.trim().to_ascii_lowercase().as_str() {
        "en" => Some(Lang::En),
        "fr" => Some(Lang::Fr),
        _ => None,
    }
}

/// Language from `P4KR_LANG`, then, on Windows (`windows_ui` is `Some`), the UI language alone: the locale
/// variables are ignored there (Git Bash sets `LANG`). Elsewhere the first set locale variable (`LC_ALL`,
/// `LC_MESSAGES`, `LANG`). English otherwise.
pub fn detect(var: &dyn Fn(&str) -> Option<String>, windows_ui: Option<u16>) -> Lang {
    if let Some(l) = var("P4KR_LANG").as_deref().and_then(parse) {
        return l;
    }
    if let Some(id) = windows_ui {
        return if id & 0x3ff == LANG_FRENCH {
            Lang::Fr
        } else {
            Lang::En
        };
    }
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| var(k).filter(|v| !v.is_empty()));
    match locale {
        Some(v) if v.to_ascii_lowercase().starts_with("fr") => Lang::Fr,
        _ => Lang::En,
    }
}

/// English count with its unit: `1 entry`, `2 entries`.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// English byte count: `1 byte`, `2 bytes`.
fn byte_count(n: u64) -> String {
    format!("{n} {}", if n == 1 { "byte" } else { "bytes" })
}

pub fn current() -> Lang {
    detect(&|k| std::env::var(k).ok(), windows_ui_language())
}

#[cfg(windows)]
fn windows_ui_language() -> Option<u16> {
    // SAFETY: no arguments, no pointers; the call only reads the user's UI language.
    Some(unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() })
}

#[cfg(not(windows))]
fn windows_ui_language() -> Option<u16> {
    None
}

// ---- Guided mode catalogue ----

/// Generates the catalogue functions that return a fixed text: `name(l) => en, fr`.
/// Also generates `PLAIN`, the list of all of them, so a new text cannot be forgotten by the tests.
macro_rules! texts {
    ($($(#[$m:meta])* $name:ident => $en:expr, $fr:expr;)*) => {
        $($(#[$m])* pub fn $name(l: Lang) -> &'static str { l.pick($en, $fr) })*

        /// Every fixed-text function of the catalogue.
        #[cfg(test)]
        pub const PLAIN: &[(&str, fn(Lang) -> &'static str)] = &[$((stringify!($name), $name)),*];
    };
}

texts! {
    menu_title => "\n1) DataForge database → XML\n2) Targeted extraction\n3) Verify the archive\nq) Quit",
        "\n1) Base DataForge → XML\n2) Extraction ciblée\n3) Vérifier l'archive\nq) Quitter";
    choice_prompt => "Choice", "Choix";
    input_unreadable => "unreadable input: guide ended", "entrée illisible : fin du guide";
    press_enter_to_close => "Press Enter to close.", "Appuie sur Entrée pour fermer.";
    none_feminine => "none", "aucune";
    none_masculine => "none", "aucun";
    saved_archive_label => "saved archive", "archive retenue";
    saved_key_label => "saved key", "clé retenue";
    saved_folder_label => "saved folder", "dossier retenu";
    archive_prompt => "Archive (.p4k)", "Archive (.p4k)";
    key_prompt_with_default => "Key file (- = no key)", "Fichier de clé (- = sans clé)";
    key_prompt_without_default => "Key file (empty or - = no key)", "Fichier de clé (vide ou - = sans clé)";
    output_prompt => "Output folder", "Dossier de sortie";
    pick_prompt => "Which one", "Laquelle";
    no_dataforge => "No DataForge database (.dcb) in the archive.", "Aucune base DataForge (.dcb) dans l'archive.";
    no_match => "No entries match.", "Aucune entrée ne correspond.";
    convert_xml_question => "Convert binary XML to text XML?", "Convertir les XML binaires en XML texte ?";
    action_dataforge => "DataForge → XML", "DataForge → XML";
    action_targeted => "Targeted extraction", "Extraction ciblée";
    action_verify => "Verification", "Vérification";
    verify_only => "verify only", "vérification seule";
    folder_not_empty => "The folder is not empty, write into it?", "Le dossier n'est pas vide, écrire dedans ?";
    launch_prompt => "Start?", "Lancer ?";
    open_folder_prompt => "Open the folder?", "Ouvrir le dossier ?";
    answer_y_or_n => "Answer y or n.", "Réponds o ou n.";
    filter_kind_prompt => "Folder (f) or extension (e)", "Dossier (d) ou extension (e)";
    answer_d_or_e => "Answer f or e.", "Réponds d ou e.";
    filter_folder_label => "Folder in the archive", "Dossier dans l'archive";
    filter_extension_label => "Extension (e.g. .xml)", "Extension (p. ex. .xml)";
    empty_answer => "Empty answer.", "Réponse vide.";
    prompt_separator => ": ", " : ";
    unit_entries => "entries", "entrées";
    unit_records => "records", "enregistrements";
    warning_prefix => "warning: ", "avertissement : ";
    record_word => "record", "enregistrement";
}

/// `[Y/n]` or `[y/N]` in English, `[O/n]` or `[o/N]` in French.
pub fn yes_no_hint(l: Lang, default: bool) -> &'static str {
    match (l, default) {
        (Lang::En, true) => "[Y/n]",
        (Lang::En, false) => "[y/N]",
        (Lang::Fr, true) => "[O/n]",
        (Lang::Fr, false) => "[o/N]",
    }
}

pub fn unknown_choice(l: Lang, choice: &str) -> String {
    format!("{} {choice}", l.pick("Unknown choice:", "Choix inconnu :"))
}

pub fn guide_interrupted(l: Lang, err: &dyn std::fmt::Display) -> String {
    format!(
        "{} {err}",
        l.pick("guide interrupted:", "guide interrompu :")
    )
}

pub fn header_fields(l: Lang, archive: &str, key: &str, folder: &str) -> String {
    match l {
        Lang::En => format!("Archive: {archive}\nKey: {key}\nFolder: {folder}"),
        Lang::Fr => format!("Archive : {archive}\nClé : {key}\nDossier : {folder}"),
    }
}

pub fn not_found(l: Lang, label: &str, path: &str) -> String {
    format!("{label} {} {path}", l.pick("not found:", "introuvable :"))
}

pub fn pick_range(l: Lang, max: usize) -> String {
    match l {
        Lang::En => format!("Enter a number from 1 to {max}."),
        Lang::Fr => format!("Numéro de 1 à {max}."),
    }
}

pub fn is_a_folder(l: Lang, path: &str) -> String {
    match l {
        Lang::En => format!("{path} is a folder: give the path of the .p4k file."),
        Lang::Fr => format!("{path} est un dossier : donne le chemin du fichier .p4k."),
    }
}

pub fn archive_found(l: Lang, path: &str) -> String {
    format!("{} {path}", l.pick("Archive found:", "Archive trouvée :"))
}

pub fn cannot_open(l: Lang, err: &dyn std::fmt::Display) -> String {
    format!("{} {err}", l.pick("cannot open:", "ouverture impossible :"))
}

pub fn key_unreadable(l: Lang, err: &dyn std::fmt::Display) -> String {
    format!("{} {err}", l.pick("unreadable key:", "clé illisible :"))
}

pub fn not_a_folder(l: Lang, path: &str) -> String {
    match l {
        Lang::En => format!("{path} is not a folder."),
        Lang::Fr => format!("{path} n'est pas un dossier."),
    }
}

pub fn action_line(l: Lang, action: &str) -> String {
    format!("{} {action}", l.pick("Action:", "Action :"))
}

pub fn archive_line(l: Lang, path: &str) -> String {
    format!("{} {path}", l.pick("Archive:", "Archive :"))
}

pub fn output_line(l: Lang, path: &str) -> String {
    format!("{} {path}", l.pick("Output:", "Sortie :"))
}

pub fn job_counts(l: Lang, entries: usize, bytes: u64, destination: &str) -> String {
    match l {
        Lang::En => format!(
            "{}, {} stored → {destination}",
            count(entries, "entry", "entries"),
            byte_count(bytes)
        ),
        Lang::Fr => format!("{entries} entrée(s), {bytes} octet(s) stocké(s) → {destination}"),
    }
}

pub fn cannot_open_folder(l: Lang, err: &dyn std::fmt::Display) -> String {
    format!(
        "{} {err}",
        l.pick(
            "cannot open the folder:",
            "impossible d'ouvrir le dossier :"
        )
    )
}

pub fn settings_not_saved(l: Lang, err: &dyn std::fmt::Display) -> String {
    format!(
        "{} {err}",
        l.pick("settings not saved:", "réglages non enregistrés :")
    )
}

pub fn extraction_refused(l: Lang, err: &dyn std::fmt::Display) -> String {
    format!(
        "{} {err}",
        l.pick("extraction refused:", "extraction refusée :")
    )
}

pub fn done_in(l: Lang, secs: f64) -> String {
    let d = format_duration(l, std::time::Duration::from_secs_f64(secs));
    match l {
        Lang::En => format!("Done in {d}."),
        Lang::Fr => format!("Terminé en {d}."),
    }
}

/// Readable duration: `9.4 s` / `9,4 s`, `2 min 44 s`, `1 h 05 min`. Rounding never yields `60.0 s`.
pub fn format_duration(l: Lang, d: std::time::Duration) -> String {
    let millis = d.as_millis();
    let tenths = (millis + 50) / 100;
    if tenths < 600 {
        let point = l.pick(".", ",");
        return format!("{}{point}{} s", tenths / 10, tenths % 10);
    }
    let secs = (millis + 500) / 1_000;
    if secs < 3_600 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{} h {:02} min", secs / 3_600, secs % 3_600 / 60)
    }
}

pub fn settings_line_expected(l: Lang, line: usize) -> String {
    match l {
        Lang::En => format!("line {line}: `key = \"value\"` expected"),
        Lang::Fr => format!("ligne {line} : `clé = \"valeur\"` attendu"),
    }
}

pub fn settings_unknown_key(l: Lang, line: usize, key: &str) -> String {
    match l {
        Lang::En => format!("line {line}: unknown key `{key}`"),
        Lang::Fr => format!("ligne {line} : clé inconnue `{key}`"),
    }
}

pub fn settings_quoted_expected(l: Lang, line: usize) -> String {
    match l {
        Lang::En => format!("line {line}: quoted value expected"),
        Lang::Fr => format!("ligne {line} : valeur entre guillemets attendue"),
    }
}

pub fn settings_unreadable(l: Lang, path: &str, err: &dyn std::fmt::Display) -> String {
    match l {
        Lang::En => format!("{path} unreadable ({err}): ignored"),
        Lang::Fr => format!("{path} illisible ({err}) : ignoré"),
    }
}

pub fn settings_invalid(l: Lang, path: &str, reason: &str) -> String {
    match l {
        Lang::En => format!("{path}: {reason}; ignored"),
        Lang::Fr => format!("{path} : {reason} ; ignoré"),
    }
}

pub fn path_not_storable(l: Lang, key: &str) -> String {
    match l {
        Lang::En => {
            format!("path `{key}` cannot be saved (not UTF-8 or contains a control character)")
        }
        Lang::Fr => {
            format!("chemin `{key}` non enregistrable (non UTF-8 ou caractère de contrôle)")
        }
    }
}

// ---- Summary lines (extraction, verification, `dcb`) ----

pub fn xml_conversion_failed(l: Lang, name: &str) -> String {
    let sep = prompt_separator(l);
    format!(
        "{name}{sep}{}",
        l.pick("XML conversion failed", "conversion XML en échec")
    )
}

pub fn dataforge_refused(l: Lang, name: &str, err: &dyn std::fmt::Display) -> String {
    let sep = prompt_separator(l);
    format!(
        "{name}{sep}{}{sep}{err}",
        l.pick("DataForge refused", "DataForge refusé")
    )
}

/// One failed record of a `.dcb`, prefixed by `name`.
pub fn record_failed(l: Lang, name: &str, digest: &str, err: &dyn std::fmt::Display) -> String {
    let sep = prompt_separator(l);
    format!("{name}{sep}{} {digest}{sep}{err}", record_word(l))
}

/// One record of a `.dcb` that hit an I/O error of the given kind.
pub fn record_io_error(l: Lang, name: &str, digest: &str, kind: &dyn std::fmt::Display) -> String {
    let sep = prompt_separator(l);
    format!(
        "{name}{sep}{} {digest}{sep}{} ({kind})",
        record_word(l),
        l.pick("I/O error", "erreur d'E/S")
    )
}

/// An entry moved aside because a DataForge record took its path.
pub fn entry_displaced(l: Lang, from: &str, to: &str) -> String {
    let sep = prompt_separator(l);
    let what = match l {
        Lang::En => format!("replaced by a DataForge record, entry kept as {to}"),
        Lang::Fr => format!("remplacé par un enregistrement DataForge, entrée gardée sous {to}"),
    };
    format!("{}{from}{sep}{what}", warning_prefix(l))
}

/// Counters of an extraction summary; `xml` exists only with `--xml`.
pub struct ExtractCounts {
    pub entries: usize,
    pub bytes: u64,
    pub errors: usize,
    pub threads: usize,
    /// `(converted, failed conversions, DataForge records)`.
    pub xml: Option<(usize, usize, usize)>,
}

pub fn extract_line(l: Lang, c: &ExtractCounts) -> String {
    let ExtractCounts {
        entries,
        bytes,
        errors,
        threads,
        xml,
    } = c;
    let base = match l {
        Lang::En => format!(
            "{}, {}, {}, {}",
            count(*entries, "entry", "entries"),
            byte_count(*bytes),
            count(*errors, "error", "errors"),
            count(*threads, "thread", "threads")
        ),
        Lang::Fr => {
            format!("{entries} entrée(s), {bytes} octet(s), {errors} erreur(s), {threads} fil(s)")
        }
    };
    let Some((converted, failed, records)) = xml else {
        return base;
    };
    match l {
        Lang::En => format!(
            "{base}, {converted} converted to XML, {}, {}",
            count(*failed, "failed conversion", "failed conversions"),
            count(*records, "DataForge record", "DataForge records")
        ),
        Lang::Fr => format!(
            "{base}, {converted} converti(s) en XML, {failed} conversion(s) en échec, {records} enregistrement(s) DataForge"
        ),
    }
}

pub fn dcb_line(l: Lang, records: usize, failed: usize, bytes: u64) -> String {
    match l {
        Lang::En => format!(
            "{} written, {failed} failed, {}",
            count(records, "record", "records"),
            byte_count(bytes)
        ),
        Lang::Fr => {
            format!("{records} enregistrement(s) écrit(s), {failed} en échec, {bytes} octet(s)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn override_wins_and_is_case_insensitive() {
        assert_eq!(
            detect(&env(&[("P4KR_LANG", "FR"), ("LANG", "en_US")]), None),
            Lang::Fr
        );
        assert_eq!(
            detect(&env(&[("P4KR_LANG", "en"), ("LANG", "fr_FR.UTF-8")]), None),
            Lang::En
        );
    }

    #[test]
    fn unknown_override_falls_back_to_the_system() {
        assert_eq!(
            detect(&env(&[("P4KR_LANG", "de"), ("LANG", "fr_CA.UTF-8")]), None),
            Lang::Fr
        );
    }

    #[test]
    fn unix_variables_in_order() {
        assert_eq!(
            detect(&env(&[("LC_ALL", "C"), ("LANG", "fr_FR.UTF-8")]), None),
            Lang::En
        );
        assert_eq!(
            detect(&env(&[("LC_MESSAGES", "fr_BE"), ("LANG", "en_GB")]), None),
            Lang::Fr
        );
        assert_eq!(detect(&env(&[("LANG", "fr")]), None), Lang::Fr);
        assert_eq!(detect(&env(&[("LANG", "")]), None), Lang::En);
        assert_eq!(detect(&env(&[]), None), Lang::En);
    }

    #[test]
    fn windows_ignores_the_locale_variables() {
        // Git Bash sets LANG on Windows: only the UI language counts there.
        assert_eq!(detect(&env(&[("LANG", "en_US")]), Some(0x040c)), Lang::Fr);
        assert_eq!(detect(&env(&[("LC_ALL", "fr_FR")]), Some(0x0409)), Lang::En);
        assert_eq!(
            detect(
                &env(&[("LANG", "en_US"), ("P4KR_LANG", "fr")]),
                Some(0x0409)
            ),
            Lang::Fr
        );
    }

    #[test]
    fn windows_ui_language() {
        assert_eq!(detect(&env(&[]), Some(0x040c)), Lang::Fr); // fr-FR
        assert_eq!(detect(&env(&[]), Some(0x0c0c)), Lang::Fr); // fr-CA
        assert_eq!(detect(&env(&[]), Some(0x0409)), Lang::En); // en-US
        assert_eq!(detect(&env(&[("P4KR_LANG", "en")]), Some(0x040c)), Lang::En);
    }

    #[test]
    fn durations_read_naturally_and_never_round_to_sixty_seconds() {
        use std::time::Duration;
        let f = |s: f64| format_duration(Lang::Fr, Duration::from_secs_f64(s));
        assert_eq!(f(0.0), "0,0 s");
        assert_eq!(f(9.43), "9,4 s");
        assert_eq!(f(59.96), "1 min 0 s");
        assert_eq!(f(59.94), "59,9 s");
        assert_eq!(f(59.95), "1 min 0 s");
        assert_eq!(f(3_599.5), "1 h 00 min");
        assert_eq!(f(3_599.4), "59 min 59 s");
        assert!(format_duration(Lang::Fr, Duration::MAX).ends_with(" min"));
        assert_eq!(f(60.0), "1 min 0 s");
        assert_eq!(f(164.0), "2 min 44 s");
        assert_eq!(f(3_600.0), "1 h 00 min");
        assert_eq!(f(3_900.0), "1 h 05 min");
        let e = |s: f64| format_duration(Lang::En, Duration::from_secs_f64(s));
        assert_eq!(e(9.43), "9.4 s");
        assert_eq!(e(164.0), "2 min 44 s");
        assert_eq!(e(3_900.0), "1 h 05 min");
    }

    #[test]
    fn french_summary_lines_keep_their_wording() {
        let c = ExtractCounts {
            entries: 0,
            bytes: 0,
            errors: 0,
            threads: 1,
            xml: None,
        };
        assert_eq!(
            extract_line(Lang::Fr, &c),
            "0 entrée(s), 0 octet(s), 0 erreur(s), 1 fil(s)"
        );
        let c = ExtractCounts {
            xml: Some((0, 0, 0)),
            ..c
        };
        assert_eq!(
            extract_line(Lang::Fr, &c),
            "0 entrée(s), 0 octet(s), 0 erreur(s), 1 fil(s), 0 converti(s) en XML, 0 conversion(s) en échec, 0 enregistrement(s) DataForge"
        );
        assert_eq!(
            dcb_line(Lang::Fr, 2, 1, 10),
            "2 enregistrement(s) écrit(s), 1 en échec, 10 octet(s)"
        );
        assert_eq!(
            entry_displaced(Lang::Fr, "o/a.xml", "o/b.xml"),
            "avertissement : o/a.xml : remplacé par un enregistrement DataForge, entrée gardée sous o/b.xml"
        );
        assert_eq!(
            record_io_error(Lang::Fr, "enregistrement", "ab12", &"x"),
            "enregistrement : enregistrement ab12 : erreur d'E/S (x)"
        );
    }

    #[test]
    fn english_counts_take_the_singular_at_one() {
        let c = ExtractCounts {
            entries: 1,
            bytes: 1,
            errors: 1,
            threads: 1,
            xml: Some((1, 1, 1)),
        };
        assert_eq!(
            extract_line(Lang::En, &c),
            "1 entry, 1 byte, 1 error, 1 thread, 1 converted to XML, 1 failed conversion, 1 DataForge record"
        );
        let c = ExtractCounts {
            entries: 2,
            bytes: 0,
            errors: 2,
            threads: 4,
            xml: Some((0, 2, 2)),
        };
        assert_eq!(
            extract_line(Lang::En, &c),
            "2 entries, 0 bytes, 2 errors, 4 threads, 0 converted to XML, 2 failed conversions, 2 DataForge records"
        );
        assert_eq!(
            dcb_line(Lang::En, 1, 0, 1),
            "1 record written, 0 failed, 1 byte"
        );
        assert_eq!(
            dcb_line(Lang::En, 2, 1, 10),
            "2 records written, 1 failed, 10 bytes"
        );
        assert_eq!(
            job_counts(Lang::En, 1, 1, "d"),
            "1 entry, 1 byte stored → d"
        );
        assert_eq!(
            job_counts(Lang::En, 3, 5, "d"),
            "3 entries, 5 bytes stored → d"
        );
    }

    #[test]
    fn done_in_uses_the_decimal_separator_of_the_language() {
        assert!(done_in(Lang::En, 9.4).contains("9.4 s"));
        assert!(done_in(Lang::Fr, 9.4).contains("9,4 s"));
    }
}
