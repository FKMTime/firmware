use anyhow::Result;
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use structs::{
    CompetitionStatusResp, Room, UnixRequest, UnixRequestData, UnixResponse, UnixResponseData,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        OnceCell, RwLock,
    },
};

mod structs;

pub static UNIX_SENDER: OnceCell<tokio::sync::mpsc::UnboundedSender<UnixResponse>> =
    OnceCell::const_new();

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

#[tokio::main]
async fn main() -> Result<()> {
    let socket_path = std::env::var("SOCKET_PATH").unwrap_or("/tmp/sock/socket.sock".to_string());
    let socket_dir = Path::new(&socket_path).parent().unwrap();
    _ = tokio::fs::create_dir_all(socket_dir).await;
    _ = tokio::fs::remove_file(&socket_path).await;

    let mut state = State {
        devices: vec![],
        cards: HashMap::new(),
        senders: Arc::new(RwLock::new(HashMap::new())),
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

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    UNIX_SENDER.set(tx)?;

    let listener = UnixListener::bind(socket_path)?;
    loop {
        let (mut stream, _) = listener.accept().await?;
        let res = handle_stream(&mut stream, &mut state, &mut rx).await;
        println!("res: {res:?}");
    }
}

async fn handle_stream(
    stream: &mut UnixStream,
    state: &mut State,
    rx: &mut UnboundedReceiver<UnixResponse>,
) -> Result<()> {
    send_status_resp(stream, &state.devices).await?;

    let mut buf = Vec::with_capacity(512);
    loop {
        tokio::select! {
            res = read_until_null(stream, &mut buf) => {
                let bytes = res?;
                let packet: UnixRequest = serde_json::from_slice(&bytes[..])?;

                match packet.data {
                    structs::UnixRequestData::RequestToConnectDevice { esp_id, .. } => {
                        state.devices.push(esp_id);
                        send_status_resp(stream, &state.devices).await?;
                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;

                        new_test_sender(&esp_id, state.senders.clone()).await?;
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
                    structs::UnixRequestData::EnterAttempt { esp_id, .. } => {
                        send_senders_data(&state.senders, &esp_id, packet.data.clone()).await?;
                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
                    }
                    _ => {
                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
                    }
                }

                println!("{packet:?}");
            }
            Some(recv) = rx.recv() => {
                send_raw_resp(stream, recv).await?;
            }
        }
    }
}

async fn new_test_sender(esp_id: &u32, senders: SharedSenders) -> Result<()> {
    tokio::task::spawn(test_sender(*esp_id, senders));

    Ok(())
}

async fn test_sender(esp_id: u32, senders: SharedSenders) -> Result<()> {
    let unix_tx = UNIX_SENDER.get().expect("UNIX_SENDER not set!");
    let rx = spawn_new_sender(&senders, esp_id).await?;

    unix_tx.send(UnixResponse {
        error: None,
        tag: None,
        data: Some(UnixResponseData::TestPacket {
            esp_id,
            data: structs::TestPacketData::Start,
        }),
    })?;

    unix_tx.send(UnixResponse {
        error: None,
        tag: None,
        data: Some(UnixResponseData::TestPacket {
            esp_id,
            data: structs::TestPacketData::ScanCard(3004425529),
        }),
    })?;

    Ok(())
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
    send_raw_resp(stream, packet).await?;

    Ok(())
}

async fn send_raw_resp(stream: &mut UnixStream, data: UnixResponse) -> Result<()> {
    stream.write_all(&serde_json::to_vec(&data)?).await?;
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

    send_raw_resp(stream, status_packet).await?;
    Ok(())
}

async fn read_until_null(stream: &mut UnixStream, buf: &mut Vec<u8>) -> Result<Vec<u8>> {
    loop {
        let byte = stream.read_u8().await?;
        if byte == 0x00 {
            let ret = buf.to_owned();
            buf.clear();

            return Ok(ret);
        }

        buf.push(byte);
    }
}

pub async fn send_senders_data(
    senders: &SharedSenders,
    esp_id: &u32,
    data: UnixRequestData,
) -> Result<()> {
    let senders = senders.read().await;
    if let Some(sender) = senders.get(esp_id) {
        sender.send(data)?;
    }

    Ok(())
}

pub async fn spawn_new_sender(
    senders: &SharedSenders,
    esp_id: u32,
) -> Result<UnboundedReceiver<UnixRequestData>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut senders = senders.write().await;
    senders.insert(esp_id, tx);

    Ok(rx)
}
