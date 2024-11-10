use alloc::string::{String, ToString};
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
    ApiError(ApiError),
    CardInfoRequest {
        card_id: u64,

        #[serde(skip_serializing_if = "Option::is_none")]
        attendance_device: Option<bool>,
    },
    CardInfoResponse(CardInfoResponsePacket),
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CardInfoResponsePacket {
    pub card_id: u64,
    pub display: String,
    pub country_iso2: String,
    pub can_compete: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApiError {
    pub error: String,
    pub should_reset_time: bool,
}

pub trait FromPacket: Sized {
    fn from_packet(packet: TimerPacket) -> Result<Self, ApiError>;
}

// TODO: macro for this shit
impl FromPacket for CardInfoResponsePacket {
    fn from_packet(packet: TimerPacket) -> Result<Self, ApiError> {
        match packet.data {
            TimerPacketInner::CardInfoResponse(card_info_response_packet) => {
                Ok(card_info_response_packet)
            }
            TimerPacketInner::ApiError(api_error) => Err(api_error),
            _ => Err(ApiError {
                error: "Wrong response type!".to_string(),
                should_reset_time: false,
            }),
        }
    }
}
