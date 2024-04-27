use anyhow::Result;
use std::{collections::HashMap, path::Path};
use structs::{CompetitionStatusResp, Room, UnixRequest, UnixResponse, UnixResponseData};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

use crate::structs::UnixRequestData;

mod structs;

pub struct State {
    pub devices: Vec<u32>,
    pub cards: HashMap<String, CompetitorInfo>,
}

pub struct CompetitorInfo {
    pub registrant_id: i64,
    pub name: String,
    pub wca_id: String,
    pub can_compete: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let socket_path = std::env::var("SOCKET_PATH").unwrap_or("/tmp/sock/socket.sock".to_string());
    let socket_dir = Path::new(&socket_path).parent().unwrap();
    _ = tokio::fs::create_dir_all(socket_dir).await;
    _ = tokio::fs::remove_file(&socket_path).await;

    let mut state = State {
        devices: vec![],
        cards: HashMap::new(),
    };
    state.cards.insert(
        "3004425529".to_string(),
        CompetitorInfo {
            name: "Filip Sciurka".to_string(),
            registrant_id: state.cards.len() as i64,
            wca_id: "FILSCI01".to_string(),
            can_compete: true,
        },
    );

    let mut device_store: Vec<u32> = vec![];

    let listener = UnixListener::bind(socket_path)?;
    loop {
        let (mut stream, _) = listener.accept().await?;
        _ = handle_stream(&mut stream, &mut state).await;
    }
}

async fn handle_stream(stream: &mut UnixStream, state: &mut State) -> Result<()> {
    send_status_resp(stream, &state.devices).await?;

    loop {
        let bytes = read_until_null(stream).await?;
        let packet: UnixRequest = serde_json::from_slice(&bytes[..])?;

        match packet.data {
            structs::UnixRequestData::RequestToConnectDevice { esp_id, .. } => {
                state.devices.push(esp_id);
                send_status_resp(stream, &state.devices).await?;
                send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
            }
            structs::UnixRequestData::PersonInfo { ref card_id } => {
                let competitor = state.cards.get(card_id);
                let resp = match competitor {
                    Some(competitor) => UnixResponseData::PersonInfoResp {
                        id: card_id.to_string(),
                        registrant_id: Some(competitor.registrant_id),
                        name: competitor.name.to_string(),
                        wca_id: Some(competitor.wca_id.to_string()),
                        country_iso2: Some("PL".to_string()),
                        gender: "Male".to_string(),
                        can_compete: competitor.can_compete,
                    },
                    None => UnixResponseData::Error {
                        message: "Competitor not found".to_string(),
                        should_reset_time: false,
                    },
                };

                send_resp(stream, resp, packet.tag, competitor.is_none()).await?;
            }
            _ => {
                send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
            }
        }

        println!("{packet:?}");
    }
}

async fn send_resp(
    stream: &mut UnixStream,
    data: UnixResponseData,
    tag: Option<u32>,
    error: bool,
) -> Result<()> {
    let packet = UnixResponse {
        tag,
        error: Some(error),
        data: Some(data),
    };

    stream.write_all(&serde_json::to_vec(&packet)?).await?;
    stream.write_u8(0x00).await?;

    Ok(())
}

async fn send_status_resp(stream: &mut UnixStream, device_store: &Vec<u32>) -> Result<()> {
    let status_packet = UnixResponse {
        tag: None,
        error: None,
        data: Some(UnixResponseData::ServerStatus(CompetitionStatusResp {
            should_update: true,
            devices: device_store.to_vec(),
            rooms: vec![Room {
                id: "dsa".to_string(),
                name: "room 1".to_string(),
                devices: device_store.to_vec(),
                use_inspection: true,
            }],
        })),
    };

    stream
        .write_all(&serde_json::to_vec(&status_packet)?)
        .await?;
    stream.write_u8(0x00).await?;

    Ok(())
}

async fn read_until_null(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut tmp = vec![];
    loop {
        let byte: u8 = stream.read_u8().await?;
        if byte == 0x00 {
            break;
        }

        tmp.push(byte);
    }

    Ok(tmp)
}
