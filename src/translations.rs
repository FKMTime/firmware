pub struct TranslationRecord<'a> {
    pub key: &'a str,
    pub translation: &'a str,
}

impl<'a> TranslationRecord<'a> {
    pub const fn new(key: &'a str, translation: &'a str) -> Self {
        TranslationRecord { key, translation }
    }
}

pub const TRANSLATION: &[TranslationRecord] = &[
    TranslationRecord::new("SCAN_COMPETITOR_1", "Scan the card"),
    TranslationRecord::new("SCAN_COMPETITOR_2", "of a competitor"),
];

pub fn get_translation(key: &str) -> Option<&str> {
    for translation in TRANSLATION {
        if translation.key == key {
            return Some(translation.translation);
        }
    }

    None
}
