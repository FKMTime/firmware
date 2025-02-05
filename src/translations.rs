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
        t.push(TranslationRecord::new(
            "SCAN_COMPETITOR_3",
            "of a competitor ({0})",
        ));

        t.push(TranslationRecord::new("SELECT_GROUP", "Select round"));

        t.push(TranslationRecord::new("CONFIRM_TIME", "Confirm the time"));
        t.push(TranslationRecord::new(
            "SCAN_JUDGE_CARD",
            "Scan the judge's card",
        ));
        t.push(TranslationRecord::new(
            "SCAN_COMPETITOR_CARD",
            "Scan the competitor's card",
        ));

        t.push(TranslationRecord::new("WIFI_WAIT_1", "Waiting for"));
        t.push(TranslationRecord::new("WIFI_WAIT_2", "WiFi connection"));

        t.push(TranslationRecord::new("MDNS_WAIT_1", "Waiting for"));
        t.push(TranslationRecord::new("MDNS_WAIT_2", "Server Discovery"));

        t.push(TranslationRecord::new("WIFI_SETUP_HEADER", "Connect to:"));

        t.push(TranslationRecord::new("DELEGATE_WAIT_HEADER", "Delegate"));
        t.push(TranslationRecord::new("DELEGATE_WAIT_TIME", "In: {0}"));

        t.push(TranslationRecord::new("DELEGATE_CALLED_1", "Waiting for"));
        t.push(TranslationRecord::new("DELEGATE_CALLED_2", "delegate"));

        t.push(TranslationRecord::new("ERROR_HEADER", "Error"));

        t.push(TranslationRecord::new(
            "DISCONNECTED_FOOTER",
            "Disconnected",
        ));

        t.push(TranslationRecord::new(
            "DEV_NOT_ADDED_HEADER",
            "Device not added",
        ));
        t.push(TranslationRecord::new(
            "DEV_NOT_ADDED_FOOTER",
            "Press submit to connect",
        ));
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
