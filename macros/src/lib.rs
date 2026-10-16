mod translations;

use proc_macro::TokenStream;

#[proc_macro]
pub fn load_default_translations(args: TokenStream) -> TokenStream {
    translations::load_translations_macro(args)
}
