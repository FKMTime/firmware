#![feature(proc_macro_span)]

mod translations;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse::{Parse, ParseStream},
    Ident, Item, Meta, Token,
};

#[derive(Debug)]
#[allow(dead_code)]
struct KeyValue {
    key: Ident,
    value_type: syn::Type,
}

#[derive(Debug)]
#[allow(dead_code)]
struct GenerateHandler {
    values: Vec<KeyValue>,
}

impl Parse for GenerateHandler {
    fn parse(input: ParseStream) -> Result<Self, syn::Error> {
        let mut values = Vec::new();
        while !input.is_empty() {
            let key = input.parse::<Ident>()?;
            input.parse::<Token![:]>()?;
            let value_type = input.parse::<syn::Type>()?;
            values.push(KeyValue { key, value_type });
            if !input.peek(Token![,]) {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        Ok(GenerateHandler { values })
    }
}

#[proc_macro]
pub fn generate_button_handler_enum(args: TokenStream) -> TokenStream {
    let input_cloned = args.clone();
    let input_parsed = syn::parse_macro_input!(input_cloned as GenerateHandler);

    let input = proc_macro2::TokenStream::from(args);
    let input_untyped_idents: Vec<_> = input_parsed
        .values
        .iter()
        .map(|kv| {
            let key = &kv.key;

            quote! {
                #key,
            }
        })
        .collect();

    let span = proc_macro::Span::call_site();
    let source_file = span.source_file();
    if !source_file.is_real() {
        panic!("Source file path not real!");
    }

    let path = source_file.path();
    let read = std::fs::read_to_string(&path);
    if let Err(_) = read {
        return quote! {
            #[doc(hidden)]
            pub enum HandlersDerive {}
        }
        .into();
    }

    let read = read.unwrap();
    let input_file = syn::parse_file(&read).unwrap();

    let mut output_ty: Option<proc_macro2::TokenStream> = None;
    let mut output_enum = Vec::new();
    let mut output_enum_impl = Vec::new();

    for item in input_file.items {
        if let Item::Fn(func) = item {
            let mut button_handler_macro = false;
            if func.attrs.len() == 1 {
                let attr = &func.attrs[0];
                if let Meta::Path(path) = &attr.meta {
                    for seg in &path.segments {
                        let ident_str = seg.ident.to_string();
                        if ident_str == "button_handler" {
                            button_handler_macro = true;
                        }
                    }
                }
            }

            if button_handler_macro {
                if func.sig.asyncness.is_none() {
                    continue;
                }
                let output = match func.sig.output {
                    syn::ReturnType::Default => quote! { () },
                    syn::ReturnType::Type(_, tp) => quote! { #tp },
                };

                if let Some(output_ty) = output_ty {
                    if output_ty.to_string() != output.to_string() {
                        panic!("Handlers result types mismatch!");
                    }
                }

                output_ty = Some(output);

                let function_name = func.sig.ident.to_string();
                let function_name = format_ident!("_button_handler_{function_name}");
                // _button_handler_name(_button_handler_name),
                output_enum.push(quote! {
                    #function_name(#function_name),
                });

                // _button_handler_enum::_button_handler_name(_button_handler_name) => _button_handler_name.execute().await,
                output_enum_impl.push(quote! {
                    Self::#function_name(#function_name) => #function_name.execute(#(#input_untyped_idents)*).await,
                });
            }
        }
    }

    let output = output_ty.unwrap_or(quote! {()});
    quote! {
        #[doc(hidden)]
        pub enum HandlersDerive {
            #(#output_enum)*
        }

        impl HandlersDerive {
            pub async fn execute(&self, #input) -> #output {
                match self {
                    #(#output_enum_impl)*
                }
            }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn button_handler(_args: TokenStream, item: TokenStream) -> TokenStream {
    let f = syn::parse_macro_input!(item as syn::ItemFn);
    if f.sig.asyncness.is_none() {
        panic!("Function has to by async!");
    }

    let inputs = f.sig.inputs;
    let output = match f.sig.output {
        syn::ReturnType::Default => quote! { () },
        syn::ReturnType::Type(_, tp) => quote! { #tp },
    };

    let name = f.sig.ident;
    let handler_name = format_ident!("_button_handler_{}", name.to_string());

    let vis = f.vis;
    let block = f.block;

    quote! {
        #[allow(non_camel_case_types)]
        #vis struct #handler_name;

        impl #handler_name {
            pub async fn execute(&self, #inputs) -> #output
                #block

        }

        #vis const fn #name() -> HandlersDerive {
            HandlersDerive::#handler_name(#handler_name)
        }
    }
    .into()
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

#[proc_macro]
pub fn load_default_translations(args: TokenStream) -> TokenStream {
    translations::load_translations_macro(args)
}

// Maybe not useful, most variables in functions that can fail are owned :(
/*
struct MacroInput {
    function_call: syn::Expr,
    retry_count: syn::LitInt,
    retry_interval: syn::LitInt,
    print_error: syn::LitBool,
    retrun_err: syn::LitBool,
}

impl Parse for MacroInput {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let function_call: syn::Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let retry_count: syn::LitInt = input.parse()?;
        input.parse::<Token![,]>()?;
        let retry_interval: syn::LitInt = input.parse()?;
        input.parse::<Token![,]>()?;
        let print_error: syn::LitBool = input.parse()?;
        input.parse::<Token![,]>()?;
        let retrun_err: syn::LitBool = input.parse()?;

        Ok(MacroInput {
            function_call,
            retry_count,
            retry_interval,
            print_error,
            retrun_err,
        })
    }
}

/// Macro to easily manage function calls that return Result and you want to retry them
///
/// Usage:
/// retry_call!(function_call, retry_count(int), retry_interval(int ms), print_error(bool), return_err(bool))
/// If return_err is false, this will panic on final error
#[proc_macro]
pub fn retry_call(item: TokenStream) -> TokenStream {
    let MacroInput {
        function_call,
        retry_count,
        retry_interval,
        print_error,
        retrun_err,
    } = syn::parse_macro_input!(item as MacroInput);

    let mut i = 0;
    let dsa = loop {
        if i % 2 == 0 {
            break i;
        }

        i += 1;
    };

    quote! {
        {
            let mut i = 0;
            loop {
                let res = #function_call;
                match res {
                    Ok(o) => {},
                    Err(e) => {
                        break Some(i);
                    }
                }

                i += 1;
            };
        }
    }
    .into()
}
*/
