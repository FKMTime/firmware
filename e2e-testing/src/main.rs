use anyhow::Result;
use std::path::Path;
use structs::{CompetitionStatusResp, UnixRequest, UnixResponse, UnixResponseData};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

mod structs;

#[tokio::main]
async fn main() -> Result<()> {
    let socket_path = std::env::var("SOCKET_PATH").unwrap_or("/tmp/sock/socket.sock".to_string());
    let socket_dir = Path::new(&socket_path).parent().unwrap();
    _ = tokio::fs::create_dir_all(socket_dir).await;
    _ = tokio::fs::remove_file(&socket_path).await;

    let mut device_store: Vec<u32> = vec![];

    let listener = UnixListener::bind(socket_path)?;
    loop {
        let (mut stream, _) = listener.accept().await?;
        _ = handle_stream(&mut stream, &mut device_store).await;
    }
}

async fn handle_stream(stream: &mut UnixStream, device_store: &mut Vec<u32>) -> Result<()> {
    send_status_resp(stream, &device_store).await?;

    loop {
        let bytes = read_until_null(stream).await?;
        let packet: UnixRequest = serde_json::from_slice(&bytes[..])?;

        match packet.data {
            structs::UnixRequestData::RequestToConnectDevice { esp_id, .. } => {
                device_store.push(esp_id);
                send_status_resp(stream, &device_store).await?;
                send_resp(stream, UnixResponseData::Empty, packet.tag).await?;
            }
            _ => {}
        }

        println!("{packet:?}");
    }
}

async fn send_resp(
    stream: &mut UnixStream,
    data: UnixResponseData,
    tag: Option<u32>,
) -> Result<()> {
    let packet = UnixResponse {
        tag,
        error: None,
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
            rooms: vec![],
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
