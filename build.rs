use std::{
    hash::Hasher,
    path::{Path, PathBuf},
};

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

    let mut hasher = crc32fast::Hasher::new();
    crc_walkdir(PathBuf::from("src"), &mut hasher);
    let src_crc = hasher.finalize();

    let mut version_str = if let Ok(rel) = std::env::var("RELEASE_BUILD") {
        rel
    } else {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if let Ok(crc_string) = std::fs::read_to_string(std::env::temp_dir().join("fkm-build-crc"))
        {
            let split = crc_string.split_once('|');
            if let Some((crc_str, ver)) = split {
                let crc: u32 = crc_str.parse().unwrap_or(0);
                if crc == src_crc {
                    ver.to_string()
                } else {
                    format!("D{epoch}")
                }
            } else {
                format!("D{epoch}")
            }
        } else {
            format!("D{epoch}")
        }
    };

    let chip = if cfg!(feature = "esp32") {
        "esp32"
    } else if cfg!(feature = "esp32c3") {
        "esp32c3"
    } else {
        "unknown"
    };

    let gen = VERSION_TEMPLATE
        .replace("{version}", &version_str)
        .replace("{chip}", chip)
        .replace("{firmware}", "STATION");

    std::fs::write(Path::new("src").join("version.rs"), gen.trim()).unwrap();
    _ = std::fs::write(
        std::env::temp_dir().join("fkm-build-crc"),
        format!("{src_crc}|{version_str}"),
    );
}

fn crc_walkdir(path: PathBuf, hasher: &mut crc32fast::Hasher) {
    if let Ok(mut dir) = path.read_dir() {
        while let Some(Ok(entry)) = dir.next() {
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                crc_walkdir(entry.path(), hasher);
            } else if file_type.is_file() && entry.file_name().to_string_lossy() != "version.rs" {
                let string = std::fs::read_to_string(entry.path()).unwrap();
                hasher.write(string.as_bytes());
            }
        }
    }
}
