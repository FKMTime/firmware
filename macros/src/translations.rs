use proc_macro::TokenStream;
use quote::{format_ident, quote};
use serde::Deserialize;
use syn::{
    parse::{Parse, ParseStream},
    Ident, LitStr, Token,
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
    name: Ident,
}

impl Parse for TranslationsHandler {
    fn parse(input: ParseStream) -> Result<Self, syn::Error> {
        let path = input.parse::<LitStr>()?;
        input.parse::<Token![,]>()?;
        let name = input.parse::<Ident>()?;

        Ok(TranslationsHandler {
            path: path.value(),
            name,
        })
    }
}

pub fn load_translations_macro(args: TokenStream) -> TokenStream {
    let TranslationsHandler { path, name } = syn::parse_macro_input!(args as TranslationsHandler);

    let read = std::fs::read(&path).unwrap();
    let translations: Vec<TranslationRecord> = serde_json::from_slice(&read).unwrap();

    let translations_count = translations.len();
    let declaration = quote! {
        pub const #name: [&'static str; #translations_count]
    };

    let mut enum_keys = Vec::new();
    let mut enum_parser_keys = Vec::new();
    let mut translations_strings = Vec::new();

    for (i, TranslationRecord { key, translation }) in translations.iter().enumerate() {
        let i = i as isize;

        let enum_key = format_ident!("{}", uppercase_first_letter(&key));
        enum_keys.push(quote! {
            #enum_key = #i,
        });

        enum_parser_keys.push(quote! {
            #key => Some(Self::#enum_key),
        });

        translations_strings.push(quote! {
            #translation,
        });
    }

    quote! {
        #[derive(Clone, Copy)]
        pub enum TranslationKey {
            #(#enum_keys)*
        }

        impl TranslationKey {
            pub fn to_usize(&self) -> usize {
                *self as usize
            }

            pub fn from_key_str(input: &str) -> Option<Self> {
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

// not performent, i know, i dont care
fn uppercase_first_letter(input: &str) -> String {
    let mut tmp = String::new();

    let mut first = true;
    for c in input.chars() {
        if first {
            tmp.push(c.to_uppercase().nth(0).unwrap());
            first = false;
        } else {
            tmp.push(c);
        }
    }

    tmp
}
