//! The app's strings (plan M10 step 6): the English table, copied in part from Lunegit's
//! `en.json` (the keys keep its shape) with Luneterm's own wording where behaviour differs. It is
//! a toolkit [`Catalog`] file, so a translation is one more file overlaid on this table.

use lune_ui::strings::{Catalog, StringTable};

const EN: &[u8] = include_bytes!("../assets/strings.en.json");

/// The English table. The file is part of the binary, so it parsing is a test, not a runtime
/// question; a failure here would leave the table empty (every lookup then falls back to its key).
pub fn english() -> StringTable {
    match Catalog::from_json(EN) {
        Ok(catalog) => catalog.into_table(),
        Err(e) => {
            eprintln!("luneterm: strings: {e:?}");
            StringTable::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{menus, settings_page};

    #[test]
    fn the_strings_file_parses_and_has_the_english_code() {
        let catalog = Catalog::from_json(EN).expect("parses");
        assert_eq!(catalog.code, "en");
        assert!(catalog.table().iter().count() > 50);
    }

    #[test]
    fn every_key_the_page_and_the_menus_ask_for_is_present() {
        let table = english();
        let mut keys = settings_page::required_keys();
        keys.extend(menus::required_keys());
        assert!(keys.len() > 60, "{} keys", keys.len());
        for key in keys {
            let text = table.get(&key);
            assert!(
                text.is_some() && (key.ends_with(".placeholder") || text != Some("")),
                "missing or empty: {key}"
            );
        }
    }

    #[test]
    fn no_user_visible_string_names_lunegit() {
        for (key, text) in english().iter() {
            assert!(
                !text.to_lowercase().contains("lunegit") && !key.to_lowercase().contains("lunegit"),
                "{key}: {text}"
            );
            assert!(!text.to_lowercase().contains("spike"), "{key}: {text}");
        }
    }

    #[test]
    fn the_restart_banner_says_what_luneterm_does() {
        let t = english();
        assert_eq!(
            t.tr("luneui.settings-page.restart-message"),
            "Shell changes apply to new terminal tabs."
        );
        assert_eq!(t.tr("luneui.settings-page.restart-action"), "Open New Tab");
    }
}
