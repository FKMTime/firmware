use alloc::string::String;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
pub struct ConnSettings {
    pub mdns: bool,
    pub ws_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TimerPacket {
    pub tag: Option<u64>,
    pub data: TimerPacketInner,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TimerPacketInner {
    StartUpdate {
        version: String,
        build_time: u64, // NOT USED
        size: i64,
        firmware: String,
    },
    Solve {
        solve_time: u64,
        penalty: i64,
        competitor_id: u64,
        judge_id: u64,
        timestamp: u64,
        session_id: String, // UUID
        delegate: bool,
        inspection_time: i64,
    },
    SolveConfirm {
        competitor_id: u64,
        session_id: String,
    },
    DelegateResponse {
        should_scan_cards: bool,

        #[serde(skip_serializing_if = "Option::is_none")]
        solve_time: Option<u64>,

        #[serde(skip_serializing_if = "Option::is_none")]
        penalty: Option<i64>,
    },
    ApiError {
        error: String,
        should_reset_time: bool,
    },
    CardInfoRequest {
        card_id: u64,

        #[serde(skip_serializing_if = "Option::is_none")]
        attendance_device: Option<bool>,
    },
    CardInfoResponse {
        card_id: u64,
        display: String,
        country_iso2: String,
        can_compete: bool,
    },
    AttendanceMarked,
    DeviceSettings {
        use_inspection: bool,
        secondary_text: String,
        added: bool,
    },
    Logs {
        //logs: Vec<LogData>,
    },
    Battery {
        level: Option<f64>,
        voltage: Option<f64>,
    },
    Add {
        firmware: String,
    },
    EpochTime {
        current_epoch: u64,
    },

    // packet for end to end testing
    //TestPacket(TestPacketData),
    //Snapshot(SnapshotData),
    //TestAck,
}
