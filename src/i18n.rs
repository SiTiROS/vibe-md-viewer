//! UI language: English (default) or Russian — `lang=ru` in settings.ini,
//! the EN/RU button on the toolbar. Strings live right in the code, in pairs:
//!
//! * `tr!("English", "Русский")` → `&'static str`;
//! * `trf!("Saved {n} files", "Сохранено файлов: {n}")` → `String` (like `format!`;
//!   substitutions are by name, so word order may differ between languages).
//!
//! The module is included first in main.rs (`#[macro_use]`), so the macros are visible in all modules.

use std::sync::atomic::{AtomicBool, Ordering};

static RU: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
        }
    }

    /// Anything other than "ru" is English.
    pub fn parse(s: &str) -> Lang {
        if s.trim().eq_ignore_ascii_case("ru") {
            Lang::Ru
        } else {
            Lang::En
        }
    }
}

pub fn set(l: Lang) {
    RU.store(l == Lang::Ru, Ordering::Relaxed);
}

pub fn is_ru() -> bool {
    RU.load(Ordering::Relaxed)
}

/// UI string in the current language: `tr!("English", "Русский")`.
macro_rules! tr {
    ($en:expr, $ru:expr $(,)?) => {
        if $crate::i18n::is_ru() {
            $ru
        } else {
            $en
        }
    };
}

/// Formatted string in the current language: `trf!("Saved {n}", "Сохранено {n}")`,
/// arguments as for `format!` (`trf!("{a} of {b}", "{a} из {b}", a = x, b = y)`).
macro_rules! trf {
    ($en:literal, $ru:literal $($rest:tt)*) => {
        if $crate::i18n::is_ru() {
            format!($ru $($rest)*)
        } else {
            format!($en $($rest)*)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default language is English. Switching is not tested here: the language is
    /// process-wide and tests run in parallel (the Russian path is covered by the self-test).
    #[test]
    fn english_by_default_and_format() {
        let n = 3;
        assert!(!is_ru());
        assert_eq!(tr!("View", "Просмотр"), "View");
        assert_eq!(trf!("{n} files", "файлов: {n}"), "3 files");
        assert_eq!(trf!("{a} of {b}", "{b}: {a}", a = 1, b = 2), "1 of 2");
        assert_eq!(Lang::parse("RU"), Lang::Ru);
        assert_eq!(Lang::parse("de"), Lang::En);
    }
}
