use alloc::string::String;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct ConnSettings {
    pub mdns: bool,
    pub ws_url: Option<String>,
}
