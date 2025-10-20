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
            .expect("Cannot fail? (Getting epoch)")
            .as_secs();

        format!("D{epoch}")
    };

    // NOTE: change this if something changes in schematic, (but not MCU)
    // This will enable firmware to be built for multiple hw revisions for example
    let hw = "v3";
    let generated = VERSION_TEMPLATE
        .replace("{version}", &version_str)
        .replace("{hw}", hw)
        .replace("{firmware}", "STATION");

    let Ok(out_dir) = std::env::var("OUT_DIR").map(PathBuf::from) else {
        panic!("Compiler should set OUT_DIR!");
    };

    std::fs::write(out_dir.join("version.rs"), generated.trim()).unwrap_or_else(|_| {
        panic!("build.rs version.rs inside outdir ({out_dir:?}) failed to write!")
    });
}
