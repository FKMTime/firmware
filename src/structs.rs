use alloc::string::String;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct ConnSettings {
    pub mdns: bool,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub secure: Option<bool>,
}
