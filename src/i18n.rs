//! Message language: English by default, Spanish when the locale is Spanish.
//!
//! Same order as gettext: the first entry of `LANGUAGE`, unless the locale
//! (`LC_ALL`, then `LC_MESSAGES`, then `LANG`) is unset, `C` or `POSIX`; then
//! that locale. `Qt.locale()`, which DMS uses for its UI language, resolves the
//! same way (measured with Quickshell 0.3.2 on 2026-11-08), so the output of
//! `check` and `binds` shown on the settings page follows the page. A language
//! picked in the DMS settings does not reach the binary.

use std::sync::OnceLock;

/// Whether messages should be in Spanish. Read once per process.
pub fn spanish() -> bool {
    static SPANISH: OnceLock<bool> = OnceLock::new();
    *SPANISH.get_or_init(|| is_spanish(|name| std::env::var(name).ok()))
}

fn is_spanish(var: impl Fn(&str) -> Option<String>) -> bool {
    let set = |name: &str| var(name).filter(|v| !v.is_empty());
    let Some(locale) = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|name| set(name)) else {
        return false;
    };
    if locale == "C" || locale.starts_with("C.") || locale == "POSIX" {
        return false;
    }
    let language = set("LANGUAGE").and_then(|l| l.split(':').find(|s| !s.is_empty()).map(str::to_owned));
    language.unwrap_or(locale).split(['_', '-', '.', '@']).next() == Some("es")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    /// The cases measured against `Qt.locale().name` in Quickshell 0.3.2.
    #[test]
    fn same_resolution_as_qt_locale() {
        assert!(is_spanish(env(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "es_MX:en")])));
        assert!(!is_spanish(env(&[("LANG", "es_MX.UTF-8"), ("LANGUAGE", "en_US:en")])));
        assert!(is_spanish(env(&[("LANG", "es_MX.UTF-8")])));
        assert!(!is_spanish(env(&[("LANG", "en_US.UTF-8")])));
        assert!(!is_spanish(env(&[("LC_ALL", "en_US.UTF-8"), ("LANG", "es_MX.UTF-8")])));
        assert!(is_spanish(env(&[("LC_ALL", "es_MX.UTF-8"), ("LANG", "en_US.UTF-8")])));
        assert!(is_spanish(env(&[("LC_MESSAGES", "es_MX.UTF-8"), ("LANG", "en_US.UTF-8")])));
        assert!(!is_spanish(env(&[("LC_ALL", "C"), ("LANGUAGE", "es_MX:en")])));
        assert!(is_spanish(env(&[("LC_ALL", "en_US.UTF-8"), ("LANGUAGE", "es_MX:en")])));
    }

    #[test]
    fn language_code_and_empty_values() {
        assert!(is_spanish(env(&[("LANG", "es")])));
        assert!(is_spanish(env(&[("LC_MESSAGES", "es_ES@euro")])));
        assert!(is_spanish(env(&[("LC_ALL", ""), ("LANG", "es_AR.UTF-8"), ("LANGUAGE", ":es")])));
        assert!(!is_spanish(env(&[("LANG", "et_EE.UTF-8")])));
        assert!(!is_spanish(env(&[("LANG", "POSIX"), ("LANGUAGE", "es")])));
        assert!(!is_spanish(env(&[("LANGUAGE", "es")])));
        assert!(!is_spanish(env(&[])));
    }
}
