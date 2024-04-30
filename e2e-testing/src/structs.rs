use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc::UnboundedSender, RwLock};
use unix_utils::request::UnixRequestData;

pub type SharedSenders = Arc<RwLock<HashMap<u32, UnboundedSender<UnixRequestData>>>>;
pub struct State {
    pub devices: Vec<u32>,
    pub cards: HashMap<String, CompetitorInfo>,
    pub senders: SharedSenders,
}

pub struct CompetitorInfo {
    pub registrant_id: i64,
    pub name: String,
    pub wca_id: String,
    pub can_compete: bool,
}
