use crate::{state::GlobalState, structs::TranslationRecord};
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::fmt::Display;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LocalLocale {
    pub locale: String,
    pub translations: Vec<Option<String>>,
}

static mut SELECTED_LOCALE: usize = usize::MAX;
static mut DEFAULT_LOCALE: usize = usize::MAX;
pub static LOCALES: Mutex<CriticalSectionRawMutex, Vec<LocalLocale>> = Mutex::new(Vec::new());
macros::load_default_translations!("src/default_translation.json");

#[allow(dead_code)]
pub fn clear_locales() {
    if let Ok(mut t) = LOCALES.try_lock() {
        t.clear();
    }
}

pub fn select_locale(locale: &str, global_state: &GlobalState) {
    let locale = locale.to_lowercase();
    let locale_idx = get_locale_index(&locale);
    select_locale_idx(locale_idx, global_state);

    log::info!("Selected locale: {locale}");
}

pub fn select_locale_idx(mut locale_idx: usize, global_state: &GlobalState) {
    unsafe {
        if locale_idx == SELECTED_LOCALE {
            return;
        }

        if locale_idx == usize::MAX {
            locale_idx = DEFAULT_LOCALE;
        }

        SELECTED_LOCALE = locale_idx;
        global_state.state.signal(); // reload locale
    }
}

pub fn set_default_locale() {
    unsafe {
        DEFAULT_LOCALE = SELECTED_LOCALE;
    }
}

pub fn restore_default_locale() {
    unsafe {
        SELECTED_LOCALE = DEFAULT_LOCALE;
    }
}

pub fn get_locale_index(locale: &str) -> usize {
    if let Ok(t) = LOCALES.try_lock() {
        t.iter()
            .enumerate()
            .find(|(_, l)| l.locale == locale)
            .map(|(i, _)| i)
            .unwrap_or(usize::MAX)
    } else {
        usize::MAX
    }
}

pub fn current_locale_index() -> usize {
    unsafe { SELECTED_LOCALE }
}

pub fn process_locale(locale: String, records: Vec<TranslationRecord>) {
    let locale = locale.to_lowercase();
    if let Ok(mut t) = LOCALES.try_lock() {
        let tmp_locale = match t.iter_mut().find(|l| l.locale == locale) {
            Some(tmp_locale) => tmp_locale,
            None => {
                let idx = t.len();
                t.push(LocalLocale {
                    locale,
                    translations: alloc::vec![None; TRANSLATIONS_COUNT],
                });

                t.get_mut(idx).expect("")
            }
        };

        for record in records {
            if let Some(key) = TranslationKey::from_key_str(&record.key) {
                tmp_locale.translations[key] = Some(record.translation);
            }
        }
    }
}

pub fn get_translation(key: usize) -> String {
    if let Ok(t) = LOCALES.try_lock() {
        if let Some(locale) = t.get(unsafe { SELECTED_LOCALE }) {
            return locale
                .translations
                .get(key)
                .map(|t| t.as_ref().unwrap_or(&"#####".to_string()).to_string())
                .unwrap_or("#####".to_string());
        } else {
            return FALLBACK_TRANSLATIONS
                .get(key)
                .map(|t| t.to_string())
                .unwrap_or("#####".to_string());
        }
    }

    "#####".to_string()
}

pub fn get_translation_params<T: Display>(key: usize, params: &[T]) -> String {
    let mut translation = get_translation(key);
    for (i, arg) in params.iter().enumerate() {
        let placeholder = alloc::format!("{{{i}}}");
        translation = translation.replace(&placeholder, &arg.to_string());
    }

    translation
}
