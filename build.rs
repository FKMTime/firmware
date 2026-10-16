use std::path::PathBuf;

const VERSION_TEMPLATE: &str = r#"
pub const VERSION: &str = "{version}";
pub const HW_VER: &str = "{hw}";
pub const FIRMWARE: &str = "{firmware}";
"#;

fn main() {
    if std::path::Path::new(".env").exists() {
        println!("cargo:rerun-if-changed=.env");
    }

    if let Ok(mut iter) = dotenvy::dotenv_iter() {
        while let Some(Ok((key, value))) = iter.next() {
            println!("cargo:rustc-env={key}={value}");
        }
    }

    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
    println!("cargo:rerun-if-env-changed=RELEASE_BUILD");
    println!("cargo:rerun-if-changed=src/");

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

    let hw = if cfg!(feature = "v4") {
        "v4"
    } else if cfg!(feature = "v3") {
        "v3"
    } else {
        "unknown"
    };

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

    std::fs::write(
        out_dir.join("translations.rs"),
        translations("src/default_translation.json"),
    )
    .unwrap_or_else(|_| panic!("build.rs translations.rs failed to write!"));
}

/// `TranslationKey` constants, key lookup and fallback texts from the default translation file.
fn translations(path: &str) -> String {
    println!("cargo:rerun-if-changed={path}");
    let json = std::fs::read(path).unwrap_or_else(|_| panic!("Cannot read {path}"));
    let records: Vec<serde_json::Value> =
        serde_json::from_slice(&json).unwrap_or_else(|e| panic!("Invalid {path}: {e}"));

    let (mut consts, mut lookup, mut texts) = (String::new(), String::new(), String::new());
    for (i, record) in records.iter().enumerate() {
        let key = record["key"]
            .as_str()
            .unwrap_or_else(|| panic!("{path}: missing key"));
        let text = record["translation"].as_str().unwrap_or_default();
        // camelCase -> CONSTANT_CASE
        let name: String = key
            .chars()
            .flat_map(|c| {
                (c.is_ascii_uppercase().then_some('_'))
                    .into_iter()
                    .chain(Some(c.to_ascii_uppercase()))
            })
            .collect();
        consts += &format!("    pub const {name}: usize = {i};\n");
        lookup += &format!("            {key:?} => Some({i}),\n");
        texts += &format!("    {text:?},\n");
    }

    format!(
        "pub struct TranslationKey;\n\n#[allow(dead_code)] // not every key is used by every hw revision\nimpl TranslationKey {{\n{consts}\n    pub fn from_key_str(input: &str) -> Option<usize> {{\n        match input {{\n{lookup}            _ => None,\n        }}\n    }}\n}}\n\npub const TRANSLATIONS_COUNT: usize = {};\npub const FALLBACK_TRANSLATIONS: [&str; TRANSLATIONS_COUNT] = [\n{texts}];\n",
        records.len()
    )
}
