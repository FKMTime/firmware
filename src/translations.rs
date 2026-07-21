use crate::{state::GlobalState, structs::TranslationRecord};
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::fmt::Display;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use portable_atomic::{AtomicUsize, Ordering};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LocalLocale {
    pub locale: String,
    pub translations: Vec<Option<String>>,
}

static SELECTED_LOCALE: AtomicUsize = AtomicUsize::new(usize::MAX);
static DEFAULT_LOCALE: AtomicUsize = AtomicUsize::new(usize::MAX);
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
    if locale_idx == SELECTED_LOCALE.load(Ordering::Relaxed) {
        return;
    }

    if locale_idx == usize::MAX {
        locale_idx = DEFAULT_LOCALE.load(Ordering::Relaxed);
    }

    SELECTED_LOCALE.store(locale_idx, Ordering::Relaxed);
    global_state.state.signal(); // reload locale
}

pub fn set_default_locale() {
    DEFAULT_LOCALE.store(SELECTED_LOCALE.load(Ordering::Relaxed), Ordering::Relaxed);
}

pub fn restore_default_locale() {
    SELECTED_LOCALE.store(DEFAULT_LOCALE.load(Ordering::Relaxed), Ordering::Relaxed);
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
    SELECTED_LOCALE.load(Ordering::Relaxed)
}

pub fn process_locale(locale: String, records: Vec<TranslationRecord>) {
    let locale = locale.to_lowercase();
    if let Ok(mut t) = LOCALES.try_lock() {
        let tmp_locale = match t.iter_mut().find(|l| l.locale == locale) {
            Some(tmp_locale) => tmp_locale,
            None => {
                t.push(LocalLocale {
                    locale,
                    translations: alloc::vec![None; TRANSLATIONS_COUNT],
                });
                // SAFETY: `last_mut` is always `Some` immediately after a push.
                match t.last_mut() {
                    Some(locale) => locale,
                    None => return,
                }
            }
        };

        for record in records {
            if let Some(key) = TranslationKey::from_key_str(&record.key) {
                tmp_locale.translations[key] = Some(record.translation);
            }
        }
    }
}

/// Placeholder shown when a translation key is missing.
const MISSING: &str = "#####";

pub fn get_translation(key: usize) -> String {
    let Ok(t) = LOCALES.try_lock() else {
        return MISSING.to_string();
    };

    if let Some(locale) = t.get(SELECTED_LOCALE.load(Ordering::Relaxed)) {
        // Selected locale present: a missing key shows the placeholder (no fallback).
        locale
            .translations
            .get(key)
            .and_then(|o| o.as_deref())
            .unwrap_or(MISSING)
            .to_string()
    } else {
        FALLBACK_TRANSLATIONS
            .get(key)
            .map(|s| s.to_string())
            .unwrap_or_else(|| MISSING.to_string())
    }
}

pub fn get_translation_params<T: Display>(key: usize, params: &[T]) -> String {
    let mut translation = get_translation(key);
    for (i, arg) in params.iter().enumerate() {
        let placeholder = alloc::format!("{{{i}}}");
        translation = translation.replace(&placeholder, &arg.to_string());
    }

    translation
}
