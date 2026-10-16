use crate::utils::bounded::{BoundedString, CappedVec};
use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

// Caps for server-provided text; longer input is truncated, never grows the heap.
pub type SessionId = BoundedString<36>; // uuid
pub type GroupId = BoundedString<48>;
pub type ShortText = BoundedString<32>;
pub type DisplayText = BoundedString<64>;
pub type MessageText = BoundedString<128>;
pub type VersionText = BoundedString<32>;
pub type FirmwareText = BoundedString<16>;
pub type LocaleCode = BoundedString<8>;
pub const MAX_POSSIBLE_GROUPS: usize = 8;
pub const MAX_BLE_DEVICES: usize = 8;

#[cfg(feature = "v3")]
pub const CONFIG_MENU_ITEMS: [&str; 6] = [
    "Reset Settings",
    "BT Display",
    "Sign Cards",
    "Un-Sign Cards",
    "Error Log",
    "Exit",
];
#[cfg(feature = "v4")]
pub const CONFIG_MENU_ITEMS: [&str; 7] = [
    "Reset Settings",
    "BT Display",
    "Sign Cards",
    "Un-Sign Cards",
    "Buzzer Volume",
    "Error Log",
    "Exit",
];

#[derive(Clone, Debug, PartialEq, Hash)]
pub struct BleDisplayDevice {
    pub name: ShortText,
    pub addr: [u8; 6],
}

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
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TimerPacketInner {
    StartUpdate {
        version: VersionText,
        build_time: u64, // NOT USED
        size: u32,
        crc: u32,
        firmware: FirmwareText,
    },
    Solve {
        solve_time: u64,
        penalty: i64,
        competitor_id: u64,
        judge_id: u64,
        timestamp: u64,
        session_id: SessionId,
        delegate: bool,
        inspection_time: i64,
        group_id: GroupId,
        sign_key: u32,
    },
    SolveConfirm(SolveConfirmPacket),
    DelegateResponse(DelegateResponsePacket),
    ApiError(ApiError),
    CustomMessage {
        line1: DisplayText,
        line2: DisplayText,
    },

    CardInfoRequest {
        card_id: u64,
        is_competitor: bool,

        #[serde(skip_serializing_if = "Option::is_none")]
        attendance_device: Option<bool>,

        sign_key: u32,
    },
    CardInfoResponse(CardInfoResponsePacket),
    AttendanceMarked,
    DeviceSettings {
        added: bool,
        // shortcut: translations are parsed into heap Vecs, then copied into the fixed
        // table (rare packet); a streaming DeserializeSeed would avoid the transient alloc
        locales: Vec<TranslationLocale>,
        default_locale: LocaleCode,
        fkm_token: i32,
        secure_rfid: bool,
        auto_setup: bool,
        sound_enabled: bool,
    },
    Battery {
        level: Option<f64>,
        voltage: Option<f64>,
    },
    Add {
        firmware: FirmwareText,
        sign_key: u32,
    },
    EpochTime {
        current_epoch: u64,
    },
    SetDeviceSettings {
        volume: Option<u8>,
    },
    DumpCrashLog,

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
    pub display: DisplayText,
    pub country_iso2: LocaleCode,
    pub can_compete: bool,
    pub possible_groups: CappedVec<PossibleGroup, MAX_POSSIBLE_GROUPS>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct PossibleGroup {
    pub group_id: GroupId,
    pub name: ShortText,
    pub secondary_text: Option<ShortText>,
    pub use_inspection: bool,
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SolveConfirmPacket {
    pub competitor_id: u64,
    pub session_id: SessionId,
    pub message: MessageText,
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
    pub error: MessageText,
    pub should_reset_time: bool,
}

/// A response type of a tagged request; an `ApiError` answer is returned as `Err`.
pub trait FromPacket: Sized {
    fn from_inner(data: TimerPacketInner) -> Option<Self>;

    fn from_packet(packet: TimerPacket) -> Result<Self, ApiError> {
        match packet.data {
            TimerPacketInner::ApiError(api_error) => Err(api_error),
            data => Self::from_inner(data).ok_or_else(|| ApiError {
                error: "Wrong response type!".into(),
                should_reset_time: false,
            }),
        }
    }
}

impl FromPacket for CardInfoResponsePacket {
    fn from_inner(data: TimerPacketInner) -> Option<Self> {
        let TimerPacketInner::CardInfoResponse(p) = data else { return None };
        Some(p)
    }
}

impl FromPacket for SolveConfirmPacket {
    fn from_inner(data: TimerPacketInner) -> Option<Self> {
        let TimerPacketInner::SolveConfirm(p) = data else { return None };
        Some(p)
    }
}

impl FromPacket for DelegateResponsePacket {
    fn from_inner(data: TimerPacketInner) -> Option<Self> {
        let TimerPacketInner::DelegateResponse(p) = data else { return None };
        Some(p)
    }
}
