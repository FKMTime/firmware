use crate::{
    state::GlobalState,
    structs::{DisplayText, LocaleCode, TranslationRecord},
    utils::text::fill_placeholders,
};
use alloc::vec::Vec;
use core::{cell::RefCell, fmt::Display};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};

pub type TranslationText = DisplayText;

/// Locales sent by the server are kept (heap, ~2 KB each) up to this many.
const MAX_LOCALES: usize = 8;
const MISSING: &str = "#####";

macros::load_default_translations!("src/default_translation.json");

struct Locale {
    code: LocaleCode,
    translations: [Option<TranslationText>; TRANSLATIONS_COUNT],
}

struct Locales {
    list: Vec<Locale>,
    /// `usize::MAX` = none (fallback translations)
    selected: usize,
    default: usize,
}

static LOCALES: Mutex<CriticalSectionRawMutex, RefCell<Locales>> =
    Mutex::new(RefCell::new(Locales {
        list: Vec::new(),
        selected: usize::MAX,
        default: usize::MAX,
    }));

fn with_locales<R>(f: impl FnOnce(&mut Locales) -> R) -> R {
    LOCALES.lock(|locales| f(&mut locales.borrow_mut()))
}

pub fn clear_locales() {
    with_locales(|l| l.list.clear());
}

pub fn select_locale(locale: &str, global_state: &GlobalState) {
    select_locale_idx(get_locale_index(locale), global_state);
    log::info!("Selected locale: {locale}");
}

pub fn select_locale_idx(locale_idx: usize, global_state: &GlobalState) {
    let changed = with_locales(|l| {
        if locale_idx == l.selected {
            return false;
        }
        l.selected = match locale_idx {
            usize::MAX => l.default,
            idx => idx,
        };
        true
    });

    if changed {
        global_state.state.signal(); // reload locale
    }
}

pub fn set_default_locale() {
    with_locales(|l| l.default = l.selected);
}

pub fn restore_default_locale() {
    with_locales(|l| l.selected = l.default);
}

/// Index of `locale` (case-insensitive), `usize::MAX` if unknown.
pub fn get_locale_index(locale: &str) -> usize {
    with_locales(|l| {
        l.list
            .iter()
            .position(|loc| loc.code.eq_ignore_ascii_case(locale))
            .unwrap_or(usize::MAX)
    })
}

pub fn current_locale_index() -> usize {
    with_locales(|l| l.selected)
}

/// Stores the server's translations for `locale` (unknown keys are ignored).
pub fn process_locale(locale: &str, records: &[TranslationRecord]) {
    with_locales(|l| {
        let idx = match l
            .list
            .iter()
            .position(|loc| loc.code.eq_ignore_ascii_case(locale))
        {
            Some(idx) => idx,
            None if l.list.len() < MAX_LOCALES => {
                let mut code = LocaleCode::truncated(locale);
                code.make_ascii_lowercase();
                l.list.push(Locale {
                    code,
                    translations: core::array::from_fn(|_| None),
                });
                l.list.len() - 1
            }
            None => {
                log::error!("Too many locales, ignoring {locale}");
                return;
            }
        };

        let translations = &mut l.list[idx].translations;
        for record in records {
            if let Some(slot) = TranslationKey::from_key_str(&record.key)
                .and_then(|key| translations.get_mut(key))
            {
                *slot = Some(TranslationText::truncated(&record.translation));
            }
        }
    });
}

pub fn get_translation(key: usize) -> TranslationText {
    with_locales(|l| match l.list.get(l.selected) {
        Some(locale) => locale
            .translations
            .get(key)
            .and_then(Clone::clone)
            .unwrap_or_else(|| MISSING.into()),
        None => FALLBACK_TRANSLATIONS
            .get(key)
            .copied()
            .unwrap_or(MISSING)
            .into(),
    })
}

/// [`get_translation`] with `{i}` replaced by `params[i]`.
pub fn get_translation_params<T: Display>(key: usize, params: &[T]) -> TranslationText {
    let mut out = TranslationText::default();
    fill_placeholders(&get_translation(key), params, &mut out);
    out
}
