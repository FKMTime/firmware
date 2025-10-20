use convert_case::Casing;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use serde::Deserialize;
use syn::{
    parse::{Parse, ParseStream},
    LitStr,
};

#[derive(Debug, Deserialize, Clone)]
pub struct TranslationRecord {
    pub key: String,
    pub translation: String,
}

#[derive(Debug)]
#[allow(dead_code)]
struct TranslationsHandler {
    path: String,
}

impl Parse for TranslationsHandler {
    fn parse(input: ParseStream) -> Result<Self, syn::Error> {
        let path = input.parse::<LitStr>()?;

        Ok(TranslationsHandler { path: path.value() })
    }
}

pub fn load_translations_macro(args: TokenStream) -> TokenStream {
    let TranslationsHandler { path } = syn::parse_macro_input!(args as TranslationsHandler);

    let read = std::fs::read(&path).expect("Cannot read translations file!");
    let translations: Vec<TranslationRecord> = serde_json::from_slice(&read).unwrap_or(Vec::new());

    let translations_count = translations.len();
    let declaration = quote! {
        pub const TRANSLATIONS_COUNT: usize = #translations_count;
        pub const FALLBACK_TRANSLATIONS: [&'static str; TRANSLATIONS_COUNT]
    };

    let mut translation_keys = Vec::new();
    let mut enum_parser_keys = Vec::new();
    let mut translations_strings = Vec::new();

    for (i, TranslationRecord { key, translation }) in translations.iter().enumerate() {
        let translation_key = format_ident!("{}", key.to_case(convert_case::Case::Constant));
        translation_keys.push(quote! {
            pub const #translation_key: usize = #i;
        });

        enum_parser_keys.push(quote! {
            #key => Some(Self::#translation_key),
        });

        translations_strings.push(quote! {
            #translation,
        });
    }

    quote! {
        pub struct TranslationKey {}

        impl TranslationKey {
            #(#translation_keys)*

            pub fn from_key_str(input: &str) -> Option<usize> {
                match input {
                    #(#enum_parser_keys)*
                    _ => None
                }
            }
        }

        #declaration = [
            #(#translations_strings)*
        ];
    }
    .into()
}
