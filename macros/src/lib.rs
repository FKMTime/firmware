use proc_macro::TokenStream;
use quote::quote;
use syn::Type;

#[proc_macro_attribute]
pub fn button_handler(_args: TokenStream, item: TokenStream) -> TokenStream {
    let f = syn::parse_macro_input!(item as syn::ItemFn);
    if f.sig.asyncness.is_none() {
        panic!("Function has to by async!");
    }

    let unnamed_inputs: Vec<Box<Type>> = f
        .sig
        .inputs
        .iter()
        .map(|i| match i {
            syn::FnArg::Receiver(receiver) => receiver.ty.clone(),
            syn::FnArg::Typed(pat_type) => pat_type.ty.clone(),
        })
        .collect();
    let unnamed_inputs = quote! {
        #(#unnamed_inputs),*
    };

    let inputs = f.sig.inputs;
    let output = match f.sig.output {
        syn::ReturnType::Default => quote! { () },
        syn::ReturnType::Type(_, tp) => quote! { #tp },
    };

    let name = f.sig.ident;
    let vis = f.vis;
    let block = f.block;

    quote! {
        #vis fn #name() -> fn(#unnamed_inputs) -> core::pin::Pin<alloc::boxed::Box<dyn core::future::Future<Output = #output> + Send>> {
            |#inputs| {
                Box::pin(async move #block)
            }
        }
    }.into()
}

#[proc_macro]
pub fn nb_to_fut(item: TokenStream) -> TokenStream {
    let item = syn::parse_macro_input!(item as syn::Expr);

    quote! {
        async {
            loop {
                match #item {
                    Ok(val) => return Ok(val),
                    Err(nb::Error::WouldBlock) => {
                        Timer::after_micros(10).await;
                        continue;
                    },
                    Err(nb::Error::Other(e)) => return Err(e)
                }
            }
        }
    }
    .into()
}
