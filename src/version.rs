#[cfg(feature = "gen_version")]
include!(concat!(env!("OUT_DIR"), "/version.rs"));

#[cfg(not(feature = "gen_version"))]
pub const VERSION: &str = "FALLBACKV";

#[cfg(not(feature = "gen_version"))]
pub const HW_VER: &str = "v0";

#[cfg(not(feature = "gen_version"))]
pub const FIRMWARE: &str = "FALLBACKF";
