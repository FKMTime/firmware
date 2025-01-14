use core::fmt::Display;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};

#[derive(Debug)]
pub struct TranslationRecord {
    pub key: String,
    pub translation: String,
}

impl TranslationRecord {
    pub fn new(key: &str, translation: &str) -> Self {
        TranslationRecord {
            key: key.to_string(),
            translation: translation.to_string(),
        }
    }
}

pub static TRANSLATIONS: Mutex<CriticalSectionRawMutex, Vec<TranslationRecord>> =
    Mutex::new(Vec::new());

pub fn init_translations() {
    if let Ok(mut t) = TRANSLATIONS.try_lock() {
        t.push(TranslationRecord::new("SCAN_COMPETITOR_1", "Scan the card"));
        t.push(TranslationRecord::new(
            "SCAN_COMPETITOR_2",
            "of a competitor",
        ));

        t.push(TranslationRecord::new("DELEGATE_WAIT", "In: {0}"));
    }
}

pub fn get_translation(key: &str) -> String {
    if let Ok(t) = TRANSLATIONS.try_lock() {
        return t
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.translation.clone())
            .unwrap_or("#####".to_string());
    }

    "#####".to_string()
}

pub fn get_translation_params<T: Display>(key: &str, params: &[T]) -> String {
    if let Ok(t) = TRANSLATIONS.try_lock() {
        let mut translation = t
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.translation.clone())
            .unwrap_or("#####".to_string());

        for (i, arg) in params.iter().enumerate() {
            let placeholder = alloc::format!("{{{}}}", i);
            translation = translation.replace(&placeholder, &arg.to_string());
        }

        return translation;
    }

    "#####".to_string()
}
