use convert_case::Casing;
use image::{GenericImageView, ImageReader};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use std::path::PathBuf;
use syn::{
    parse::{Parse, ParseStream},
    LitStr,
};

#[derive(Debug)]
#[allow(dead_code)]
struct PixelArtHandler {
    path: String,
}

impl Parse for PixelArtHandler {
    fn parse(input: ParseStream) -> Result<Self, syn::Error> {
        let path = input.parse::<LitStr>()?;

        Ok(PixelArtHandler { path: path.value() })
    }
}

pub fn load_lcd_resources(args: TokenStream) -> TokenStream {
    let PixelArtHandler { path } = syn::parse_macro_input!(args as PixelArtHandler);

    let mut resources = Vec::new();
    let mut res = std::fs::read_dir(path).expect("Cannot read specified path");
    while let Some(Ok(e)) = res.next() {
        let metadata = e.metadata().unwrap();
        if metadata.is_file() {
            if let Ok((width, height, data)) = parse_img(e.path()) {
                let name = format_ident!(
                    "{}",
                    e.file_name()
                        .to_string_lossy()
                        .split(".")
                        .next()
                        .unwrap()
                        .to_case(convert_case::Case::Constant)
                );
                let data_tokens = quote! { &[#(#data),*] };

                resources.push(quote! {
                    pub const #name: PixelArt = PixelArt::new(#data_tokens, #width, #height);
                });
            }
        }
    }

    quote! {
        pub struct Resources {}

        impl Resources {
            #(#resources)*
        }
    }
    .into()
}

fn parse_img(path: PathBuf) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let img = ImageReader::open(path)?.decode()?;
    let (w, h) = img.dimensions();

    let mut bounding_box_left = 0;
    let mut bounding_box_right = w;
    let mut bounding_box_top = h;
    let mut bounding_box_bottom = 0;

    for x in 0..w {
        let mut all_clear = true;
        for y in 0..h {
            let px = img.get_pixel(x, y).0;
            if px == [0, 0, 0, 255] {
                all_clear = false;
                break;
            }
        }

        if !all_clear {
            bounding_box_right = x;
        }
    }

    for x in (0..w).rev() {
        let mut all_clear = true;
        for y in 0..h {
            let px = img.get_pixel(x, y).0;
            if px == [0, 0, 0, 255] {
                all_clear = false;
                break;
            }
        }

        if !all_clear {
            bounding_box_left = x;
        }
    }

    for y in 0..h {
        let mut all_clear = true;
        for x in 0..w {
            let px = img.get_pixel(x, y).0;
            if px == [0, 0, 0, 255] {
                all_clear = false;
                break;
            }
        }

        if !all_clear {
            bounding_box_top = y;
        }
    }

    for y in (0..h).rev() {
        let mut all_clear = true;
        for x in 0..w {
            let px = img.get_pixel(x, y).0;
            if px == [0, 0, 0, 255] {
                all_clear = false;
                break;
            }
        }

        if !all_clear {
            bounding_box_bottom = y;
        }
    }

    let x_size = bounding_box_right - bounding_box_left + 1;
    let y_size = bounding_box_top - bounding_box_bottom + 1;

    let bytes_per_row = (x_size + 7) / 8;
    let mut data = vec![0u8; (bytes_per_row * y_size) as usize];
    for y in bounding_box_bottom..=bounding_box_top {
        for x in bounding_box_left..=bounding_box_right {
            let px = img.get_pixel(x, y).0;

            if px == [0, 0, 0, 255] {
                let relative_x = x - bounding_box_left;
                let relative_y = y - bounding_box_bottom;

                let byte_idx = (relative_y * bytes_per_row + relative_x / 8) as usize;
                let bit_idx = 7 - (relative_x % 8);
                data[byte_idx] |= 1 << bit_idx;
            }
        }
    }

    Ok((x_size, y_size, data))
}
