use proc_macro::TokenStream;
use quote::quote;

#[proc_macro_attribute]
pub fn button_handler(_args: TokenStream, item: TokenStream) -> TokenStream {
    let f = syn::parse_macro_input!(item as syn::ItemFn);
    if f.sig.asyncness.is_none() {
        panic!("Function has to by async!");
    }

    let inputs = f.sig.inputs;
    let output = f.sig.output;
    let name = f.sig.ident;
    let vis = f.vis;
    let block = f.block;

    quote! {
        #vis fn #name() -> fn(ButtonTrigger, u64) -> core::pin::Pin<alloc::boxed::Box<dyn core::future::Future<Output = Result<(), ()>> + Send>> {
            |#inputs| {
                Box::pin(async move #block)
            }
        }
    }.into()
}
