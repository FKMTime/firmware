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
/*
pub const TRANSLATION: &[TranslationRecord] = &[
    TranslationRecord {
        key: String::from("dsa"),
        translation: "".to_string(),
    }, //TranslationRecord::new("SCAN_COMPETITOR_1", "Scan the card"),
       //TranslationRecord::new("SCAN_COMPETITOR_2", "of a competitor"),
];
*/

pub fn init_translations() {
    if let Ok(mut t) = TRANSLATIONS.try_lock() {
        t.push(TranslationRecord::new("SCAN_COMPETITOR_1", "Scan the card"));
        t.push(TranslationRecord::new(
            "SCAN_COMPETITOR_2",
            "of a competitor",
        ));
    }
}

pub fn get_translation(key: &str) -> Option<String> {
    if let Ok(t) = TRANSLATIONS.try_lock() {
        return t
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.translation.clone());
    }

    None
}
