use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
pub struct ConnSettings {
    pub mdns: bool,
    pub ws_url: Option<String>,
}

impl Default for ConnSettings {
    fn default() -> Self {
        Self {
            mdns: true,
            ws_url: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TimerPacket {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<u64>,
    pub data: TimerPacketInner,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sign_key: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TimerPacketInner {
    StartUpdate {
        version: String,
        build_time: u64, // NOT USED
        size: u32,
        crc: u32,
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
        group_id: String,
    },
    SolveConfirm(SolveConfirmPacket),
    DelegateResponse(DelegateResponsePacket),
    ApiError(ApiError),
    CustomMessage {
        line1: String,
        line2: String,
    },

    CardInfoRequest {
        card_id: u64,

        #[serde(skip_serializing_if = "Option::is_none")]
        attendance_device: Option<bool>,
    },
    CardInfoResponse(CardInfoResponsePacket),
    AttendanceMarked,
    DeviceSettings {
        added: bool,
        locales: Vec<TranslationLocale>,
        default_locale: String,
    },
    Logs {
        logs: Vec<String>,
    },
    Battery {
        level: Option<f64>,
        voltage: Option<f64>,
    },
    Add {
        firmware: String,
        sign_key: u32,
    },
    EpochTime {
        current_epoch: u64,
    },
    // packet for end to end testing
    #[cfg(feature = "e2e")]
    TestPacket(TestPacketData),

    #[cfg(feature = "e2e")]
    TestAck(SnapshotData),
}

#[cfg(feature = "e2e")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TestPacketData {
    HardStateReset,
    ResetState,
    ScanCard(u64),
    ButtonPress { pin: u8, press_time: u64 },
    StackmatTime(u64),
    StackmatReset,
}

#[cfg(feature = "e2e")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SnapshotData {
    pub scene: usize,
    pub inspection_time: Option<u64>,
    pub solve_time: Option<u64>,
    pub penalty: Option<i8>,
    pub time_confirmed: bool,
    pub possible_groups: usize,
    pub group_selected_idx: usize,
    pub current_competitor: Option<u64>,
    pub current_judge: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranslationLocale {
    pub locale: String,
    pub translations: Vec<TranslationRecord>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranslationRecord {
    pub key: String,
    pub translation: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CardInfoResponsePacket {
    pub card_id: u64,
    pub display: String,
    pub country_iso2: String,
    pub can_compete: bool,
    pub possible_groups: Vec<PossibleGroup>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PossibleGroup {
    pub group_id: String,
    pub secondary_text: String,
    pub use_inspection: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SolveConfirmPacket {
    pub competitor_id: u64,
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DelegateResponsePacket {
    pub should_scan_cards: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub solve_time: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub penalty: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApiError {
    pub error: String,
    pub should_reset_time: bool,
}

pub trait FromPacket: Sized {
    fn from_packet(packet: TimerPacket) -> Result<Self, ApiError>;
}

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

impl FromPacket for SolveConfirmPacket {
    fn from_packet(packet: TimerPacket) -> Result<Self, ApiError> {
        match packet.data {
            TimerPacketInner::SolveConfirm(solve_confirm_packet) => Ok(solve_confirm_packet),
            TimerPacketInner::ApiError(api_error) => Err(api_error),
            _ => Err(ApiError {
                error: "Wrong response type!".to_string(),
                should_reset_time: false,
            }),
        }
    }
}

impl FromPacket for DelegateResponsePacket {
    fn from_packet(packet: TimerPacket) -> Result<Self, ApiError> {
        match packet.data {
            TimerPacketInner::DelegateResponse(delegate_response_packet) => {
                Ok(delegate_response_packet)
            }
            TimerPacketInner::ApiError(api_error) => Err(api_error),
            _ => Err(ApiError {
                error: "Wrong response type!".to_string(),
                should_reset_time: false,
            }),
        }
    }
}
