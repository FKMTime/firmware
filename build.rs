use std::path::Path;

const VERSION_TEMPLATE: &'static str = r#"
pub const VERSION: &'static str = "{version}";
pub const CHIP: &'static str = "{chip}";
pub const FIRMWARE: &'static str = "{firmware}";
"#;

fn main() {
    println!("cargo:rerun-if-changed=*.env*");
    if let Ok(mut iter) = dotenvy::dotenv_iter() {
        while let Some(Ok((key, value))) = iter.next() {
            println!("cargo:rustc-env={key}={value}");
        }
    }

    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");

    let version_str = if let Ok(rel) = std::env::var("RELEASE_BUILD") {
        rel
    } else {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        format!("D{epoch}")
    };

    #[cfg(feature = "esp32")]
    let chip = { "esp32" };

    #[cfg(feature = "esp32c3")]
    let chip = { "esp32c3" };

    let gen = VERSION_TEMPLATE
        .replace("{version}", &version_str)
        .replace("{chip}", chip)
        .replace("{firmware}", "STATION");

    std::fs::write(Path::new("src").join("version.rs"), gen.trim()).unwrap();
}
