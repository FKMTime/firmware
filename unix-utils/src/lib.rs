use serde::{Deserialize, Serialize};

pub mod request;
pub mod response;

#[derive(Debug)]
pub struct UnixError {
    pub message: String,
    pub should_reset_time: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TestPacketData {
    Start,
    End,
    ResetState,
    ScanCard(u64),
    ButtonPress { pins: Vec<u8>, press_time: u64 },
    SolveTime(u64),
    Snapshot,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SnapshotData {
    pub esp_id: u32,
    pub scene: u32,
    pub solve_session_id: String,
    pub solve_time: i64,
    pub last_solve_time: i64,
    pub penalty: i64,
    pub use_inspection: bool,
    pub inspection_started: u64,
    pub inspection_ended: u64,
    pub competitor_card_id: u64,
    pub judge_card_id: u64,
    pub competitor_display: String,
    pub time_confirmed: bool,
    pub error_msg: String,
}
