use std::path::PathBuf;

const VERSION_TEMPLATE: &str = r#"
pub const VERSION: &str = "{version}";
pub const HW_VER: &str = "{hw}";
pub const FIRMWARE: &str = "{firmware}";
"#;

fn main() {
    println!("cargo:rerun-if-changed=*.env*");
    if let Ok(mut iter) = dotenvy::dotenv_iter() {
        while let Some(Ok((key, value))) = iter.next() {
            println!("cargo:rustc-env={key}={value}");
        }
    }

    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
    println!("cargo:rustc-cfg=feature=\"gen_version\"");

    let version_str = if let Ok(rel) = std::env::var("RELEASE_BUILD") {
        println!("cargo:rustc-cfg=feature=\"release_build\"");
        rel
    } else {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        format!("D{epoch}")
    };

    let hw = if cfg!(feature = "esp32") {
        "v2"
    } else if cfg!(feature = "esp32c3") {
        "v3"
    } else {
        "unknown"
    };

    let generated = VERSION_TEMPLATE
        .replace("{version}", &version_str)
        .replace("{hw}", hw)
        .replace("{firmware}", "STATION");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out_dir.join("version.rs"), generated.trim()).unwrap();
}
