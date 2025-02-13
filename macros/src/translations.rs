use proc_macro::TokenStream;
use quote::quote;
use serde::Deserialize;
use syn::{
    parse::{Parse, ParseStream},
    Ident, LitStr, Token,
};

#[derive(Debug, Deserialize)]
pub struct TranslationRecord {
    pub key: String,
    pub translation: String,
}

#[derive(Debug)]
#[allow(dead_code)]
struct TranslationsHandler {
    path: String,
    name: Ident,
    typ: syn::Type,
}

impl Parse for TranslationsHandler {
    fn parse(input: ParseStream) -> Result<Self, syn::Error> {
        let path = input.parse::<LitStr>()?;
        input.parse::<Token![,]>()?;
        let name = input.parse::<Ident>()?;
        input.parse::<Token![,]>()?;
        let typ = input.parse::<syn::Type>()?;

        Ok(TranslationsHandler {
            path: path.value(),
            name,
            typ,
        })
    }
}

pub fn load_translations_macro(args: TokenStream) -> TokenStream {
    let TranslationsHandler { path, name, typ } =
        syn::parse_macro_input!(args as TranslationsHandler);

    let read = std::fs::read(&path).unwrap();
    let translations: Vec<TranslationRecord> = serde_json::from_slice(&read).unwrap();

    let translations_count = translations.len();
    let declaration = quote! {
        pub const #name: [#typ; #translations_count]
    };

    let translations = translations
        .into_iter()
        .map(|t| {
            let key = t.key;
            let translation = t.translation;

            quote! {
                #typ {
                    key: #key,
                    translation: #translation,
                },
            }
        })
        .collect::<Vec<_>>();

    quote! {
        #declaration = [
            #(#translations)*
        ];
    }
    .into()
}
