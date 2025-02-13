use core::fmt::Display;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use serde::Deserialize;

use crate::utils::normalize_polish_letters;

#[derive(Debug)]
#[allow(dead_code)]
pub struct StaticTranslationRecord {
    pub key: &'static str,
    pub translation: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct TranslationLocale {
    pub locale: String,
    pub translations: Vec<TranslationRecord>,
}

#[derive(Debug, Deserialize)]
pub struct TranslationRecord {
    pub key: String,
    pub translation: String,
}

#[allow(dead_code)]
impl TranslationRecord {
    pub fn new(key: &str, translation: &str) -> Self {
        TranslationRecord {
            key: key.to_string(),
            translation: translation.to_string(),
        }
    }
}

macros::load_default_translations!("src/default_translation.json", FALLBACK_TRANSLATIONS);

pub static TRANSLATIONS: Mutex<CriticalSectionRawMutex, Vec<TranslationLocale>> =
    Mutex::new(Vec::new());

pub fn init_translations() {
    if let Ok(mut t) = TRANSLATIONS.try_lock() {
        t.push(TranslationLocale {
            locale: "pl".to_string(),
            translations: serde_json::from_str::<Vec<TranslationRecord>>(include_str!(
                "locale_pl_test.json"
            ))
            .unwrap()
            .into_iter()
            .map(|t| TranslationRecord {
                key: t.key,
                translation: normalize_polish_letters(t.translation),
            })
            .collect(),
        });
    }
}

pub fn get_translation(key: &str) -> String {
    if let Ok(t) = TRANSLATIONS.try_lock() {
        if let Some(locale) = t.iter().find(|l| l.locale == "pl") {
            return locale
                .translations
                .iter()
                .find(|t| t.key == key)
                .map(|t| t.translation.clone())
                .unwrap_or("#####".to_string());
        } else {
            /*
            return FALLBACK_TRANSLATIONS
                .iter()
                .find(|t| t.key == key)
                .map(|t| t.translation.to_string())
                .unwrap_or("#####".to_string());
            */
        }
    }

    "#####".to_string()
}

pub fn get_translation_params<T: Display>(key: &str, params: &[T]) -> String {
    let mut translation = get_translation(key);
    for (i, arg) in params.iter().enumerate() {
        let placeholder = alloc::format!("{{{}}}", i);
        translation = translation.replace(&placeholder, &arg.to_string());
    }

    translation
}
