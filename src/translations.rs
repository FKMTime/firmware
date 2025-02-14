use crate::{state::GlobalState, structs::TranslationRecord, utils::normalize_polish_letters};
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

// TODO: make selected locale as index to LOCALES
static mut SELECTED_LOCALE: [char; 6] = ['\0'; 6];
static mut LOCALE_LENGTH: usize = 0;

pub static LOCALES: Mutex<CriticalSectionRawMutex, Vec<LocalLocale>> = Mutex::new(Vec::new());
macros::load_default_translations!("src/default_translation.json");

#[allow(dead_code)]
pub fn clear_locales() {
    if let Ok(mut t) = LOCALES.try_lock() {
        t.clear();
    }
}

pub fn select_locale(locale: &str, global_state: &GlobalState) {
    let selected_locale = unsafe { SELECTED_LOCALE[..LOCALE_LENGTH].iter().collect::<String>() };
    if selected_locale == locale {
        return;
    }

    if locale.chars().count() > 6 {
        log::error!("Locale too long!");
        return;
    }

    unsafe {
        LOCALE_LENGTH = 0;
        for (i, c) in locale.chars().enumerate() {
            SELECTED_LOCALE[i] = c;
            LOCALE_LENGTH = i + 1;
        }
    }

    global_state.state.signal(); // reload locale
    log::info!("Selected locale: {locale}");
}

pub fn process_locale(locale: String, records: Vec<TranslationRecord>) {
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
                tmp_locale.translations[key] = Some(normalize_polish_letters(record.translation));
            }
        }
    }
}

pub fn get_translation(key: usize) -> String {
    let selected_locale = unsafe { SELECTED_LOCALE[..LOCALE_LENGTH].iter().collect::<String>() };

    if let Ok(t) = LOCALES.try_lock() {
        if let Some(locale) = t.iter().find(|l| l.locale == selected_locale) {
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
        let placeholder = alloc::format!("{{{}}}", i);
        translation = translation.replace(&placeholder, &arg.to_string());
    }

    translation
}
