//! Shared English/Spanish UI catalog and per-user language preference.
use std::collections::HashMap;
use std::sync::OnceLock;

macro_rules! trf {
    ($en:literal, $es:literal $(, $arg:expr)* $(,)?) => {
        if crate::i18n::spanish() { format!($es $(, $arg)*) } else { format!($en $(, $arg)*) }
    };
}

fn preference_path() -> std::path::PathBuf {
    gtk::glib::user_config_dir().join("termica/language")
}

pub fn language() -> String {
    std::fs::read_to_string(preference_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| matches!(s.as_str(), "en" | "es" | "system"))
        .unwrap_or_else(|| "en".into())
}

pub fn set_language(value: &str) -> std::io::Result<()> {
    let path = preference_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, value)
}

pub fn spanish() -> bool {
    static SPANISH: OnceLock<bool> = OnceLock::new();
    *SPANISH.get_or_init(|| match language().as_str() {
        "es" => true,
        "en" => false,
        _ => gtk::glib::language_names().first().is_some_and(|s| is_spanish(s)),
    })
}

fn is_spanish(locale: &str) -> bool {
    locale.split(['_', '-', '.', '@']).next().is_some_and(|s| s.eq_ignore_ascii_case("es"))
}

pub fn tr(text: &str) -> &str {
    static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
    if spanish() {
        CATALOG
            .get_or_init(|| {
                serde_json::from_str(include_str!("../../../extension/termica@ikario/es.json"))
                    .expect("valid Spanish catalog")
            })
            .get(text)
            .map(String::as_str)
            .unwrap_or(text)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_spanish_locales() {
        for locale in ["es", "es_SV.UTF-8", "es-ES", "es_MX@latin"] {
            assert!(is_spanish(locale));
        }
        for locale in ["en_US", "C", "fr", "estonian"] {
            assert!(!is_spanish(locale));
        }
    }

    #[test]
    fn catalog_is_valid() {
        let catalog: HashMap<String, String> =
            serde_json::from_str(include_str!("../../../extension/termica@ikario/es.json")).unwrap();
        assert_eq!(catalog["Save"], "Guardar");
    }
}
