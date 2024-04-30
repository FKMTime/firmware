use serde::{Deserialize, Serialize};

// ALL THIS THINGS ARE COPIED FROM BACKEND
// TODO: make them as separate lib/crate

#[derive(Debug)]
pub struct UnixError {
    pub message: String,
    pub should_reset_time: bool,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct UnixResponse {
    pub error: Option<bool>,
    pub tag: Option<u32>,

    #[serde(flatten)]
    pub data: Option<UnixResponseData>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all_fields = "camelCase")]
pub enum UnixResponseData {
    WifiSettingsResp {
        wifi_ssid: String,
        wifi_password: String,
    },
    ServerStatus(CompetitionStatusResp),
    PersonInfoResp {
        id: String,
        registrant_id: Option<i64>,
        name: String,
        wca_id: Option<String>,
        country_iso2: Option<String>,
        gender: String,
        can_compete: bool,
    },
    Error {
        message: String,
        should_reset_time: bool,
    },
    Success {
        message: String,
    },
    IncidentResolved {
        esp_id: u32,
        should_scan_cards: bool,
        attempt: IncidentAttempt,
    },
    TestPacket {
        esp_id: u32,
        data: TestPacketData,
    },
    Empty,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct UnixRequest {
    pub tag: Option<u32>,

    #[serde(flatten)]
    pub data: UnixRequestData,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all_fields = "camelCase")]
pub enum UnixRequestData {
    PersonInfo {
        card_id: String,
    },
    WifiSettings,
    CreateAttendance {
        card_id: u64,
        esp_id: u32,
    },
    EnterAttempt {
        value: u64,
        penalty: i64,
        solved_at: String,
        esp_id: u32,
        judge_id: String,
        competitor_id: String,
        is_delegate: bool,
        session_id: String,
        inspection_time: u64,
    },
    UpdateBatteryPercentage {
        esp_id: u32,
        battery_percentage: u8,
    },
    RequestToConnectDevice {
        esp_id: u32,

        #[serde(rename = "type")]
        r#type: String,
    },
    Snapshot(SnapshotData),
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IncidentAttempt {
    pub session_id: String,
    pub penalty: i64,
    pub value: u64,
    pub inspection_time: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TestPacketData {
    Start,
    End,
    ResetState,
    ScanCard(u64),
    ButtonPress { pins: Vec<u8>, press_time: u64 },
    SolveTime(u64),
    Snapshot
}

#[derive(Debug, Clone)]
pub struct CompetitionDeviceSettings {
    pub use_inspection: Option<bool>,
}

// API RESPONSE
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitionStatusResp {
    pub should_update: bool,
    pub devices: Vec<u32>,
    pub rooms: Vec<Room>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub name: String,
    pub use_inspection: bool,
    pub devices: Vec<u32>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
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
