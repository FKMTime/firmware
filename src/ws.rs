use crate::{
    consts::WS_RETRY_MS,
    state::{GlobalState, Scene},
    structs::{ApiError, FromPacket, TimerPacket, TimerPacketInner},
};
use alloc::string::String;
use core::str::FromStr;
use embassy_net::{
    tcp::{TcpReader, TcpSocket, TcpWriter},
    IpAddress, Stack,
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, pubsub::PubSubChannel,
};
use embassy_time::{Instant, Timer};
use embedded_io_async::Write;
use embedded_tls::{Aes128GcmSha256, NoVerify, TlsConfig, TlsConnection, TlsContext};
use esp_hal_ota::Ota;
use esp_storage::FlashStorage;
use rand_core::OsRng;
use ws_framer::{WsFrame, WsFrameOwned, WsRxFramer, WsTxFramer, WsUrl};

static FRAME_CHANNEL: Channel<CriticalSectionRawMutex, WsFrameOwned, 10> = Channel::new();
static TAGGED_RETURN: PubSubChannel<CriticalSectionRawMutex, (u64, TimerPacket), 20, 20, 4> =
    PubSubChannel::new();

#[embassy_executor::task]
pub async fn ws_task(
    stack: Stack<'static>,
    ws_url: String,
    global_state: GlobalState,
    sha: esp_hal::peripherals::SHA,
    rsa: esp_hal::peripherals::RSA,
) {
    let ws_url = WsUrl::from_str(&ws_url).unwrap();

    let mut rx_buffer = [0; 8192];
    let mut tx_buffer = [0; 8192];

    let mut ws_rx_buf = alloc::vec![0; 8192];
    let mut ws_tx_buf = alloc::vec![0; 8192];

    loop {
        {
            global_state.state.lock().await.server_connected = Some(false);
        }

        let ip = if let Ok(addr) = embassy_net::Ipv4Address::from_str(ws_url.ip) {
            addr
        } else {
            let dns_resolver = embassy_net::dns::DnsSocket::new(stack);
            let res = dns_resolver
                .query(ws_url.ip, embassy_net::dns::DnsQueryType::A)
                .await;
            if let Err(e) = res {
                log::error!("[WS]Dns resolver error: {e:?}");
                Timer::after_millis(1000).await;
                continue;
            }

            let res = res.unwrap();
            let first = res.first();
            if first.is_none() {
                log::error!("[WS]Dns resolver empty vec");
                Timer::after_millis(1000).await;
                continue;
            }

            let IpAddress::Ipv4(addr) = first.unwrap();
            *addr
        };

        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(embassy_time::Duration::from_secs(15)));

        let remote_endpoint = (ip, ws_url.port);
        let r = socket.connect(remote_endpoint).await;
        if let Err(e) = r {
            log::error!("connect error: {:?}", e);
            Timer::after_millis(WS_RETRY_MS).await;
            continue;
        }

        let mut read_record_buffer = [0; 16640];
        let mut write_record_buffer = [0; 16640];
        let config: TlsConfig<'_, Aes128GcmSha256> = TlsConfig::new().with_server_name(ws_url.host);
        let mut tls = TlsConnection::new(socket, &mut read_record_buffer, &mut write_record_buffer);

        tls.open::<OsRng, NoVerify>(TlsContext::new(&config, &mut OsRng))
            .await
            .expect("error establishing TLS connection");

        {
            global_state.state.lock().await.server_connected = Some(true);
        }
        log::info!("connected!");
        let mut tx_framer = WsTxFramer::new(true, &mut ws_tx_buf);
        let mut rx_framer = WsRxFramer::new(&mut ws_rx_buf);

        let path = alloc::format!(
            "{}?id={}&ver={}&chip={}&firmware={}",
            ws_url.path,
            crate::utils::get_efuse_u32(),
            crate::version::VERSION,
            crate::version::CHIP,
            crate::version::FIRMWARE,
        );

        tls.write_all(tx_framer.generate_http_upgrade(ws_url.host, &path, None))
            .await
            .unwrap();
        tls.flush().await.unwrap();

        loop {
            let n = tls.read(rx_framer.mut_buf()).await.unwrap();
            let res = rx_framer.process_http_response(n);

            if let Some(code) = res {
                log::info!("http_resp_code: {code}");
                break;
            }
        }

        FRAME_CHANNEL
            .send(WsFrameOwned::Ping(alloc::vec::Vec::new()))
            .await;

        loop {
            let res = ws_rw(
                &mut rx_framer,
                &mut tx_framer,
                global_state.clone(),
                &mut tls,
            )
            .await;

            if res.is_err() {
                log::error!("ws: reader or writer err!");
                Timer::after_millis(WS_RETRY_MS).await;
                break;
            }
        }

        loop {
            Timer::after_millis(100).await;
        }
    }
}

async fn ws_rw(
    framer_rx: &mut WsRxFramer<'_>,
    framer_tx: &mut WsTxFramer<'_>,
    global_state: GlobalState,
    tls: &mut TlsConnection<'_, TcpSocket<'_>, Aes128GcmSha256>,
) -> Result<(), ()> {
    let mut ota = Ota::new(FlashStorage::new()).map_err(|_| ())?;
    let tagged_publisher = TAGGED_RETURN.publisher().map_err(|_| ())?;
    let recv = FRAME_CHANNEL.receiver();

    loop {
        let read_fut = tls.read(framer_rx.mut_buf());
        let write_fut = recv.receive();

        let res = match embassy_futures::select::select(read_fut, write_fut).await {
            embassy_futures::select::Either::First(read_res) => read_res,
            embassy_futures::select::Either::Second(write_frame) => {
                let data = framer_tx.frame(write_frame.into_ref());
                tls.write_all(data).await.map_err(|_| ())?;
                tls.flush().await.map_err(|_| ())?;

                continue;
            }
        };

        if let Err(e) = res {
            log::error!("ws_read: {e:?}");
            return Err(());
        }

        let n = res.unwrap();
        if n == 0 {
            log::warn!("read_n: 0");
            return Err(());
        }

        framer_rx.revolve_write_offset(n);
        while let Some(frame) = framer_rx.process_data() {
            //log::warn!("recv_frame: opcode:{}", frame.opcode());

            match frame {
                WsFrame::Text(text) => match serde_json::from_str::<TimerPacket>(text) {
                    Ok(timer_packet) => {
                        //log::info!("Timer packet recv: {timer_packet:?}");
                        if let Some(tag) = timer_packet.tag {
                            tagged_publisher.publish((tag, timer_packet.clone())).await;
                        }

                        match timer_packet.data {
                            TimerPacketInner::DeviceSettings {
                                use_inspection,
                                secondary_text,
                                added,
                            } => {
                                let mut state = global_state.state.lock().await;
                                state.use_inspection = use_inspection;
                                state.device_added = Some(added);
                                state.secondary_text = Some(secondary_text);
                            }
                            TimerPacketInner::ApiError(e) => {
                                // if should_reset_time reset time
                                let mut state = global_state.state.lock().await;
                                state.error_text = Some(e.error);
                            }
                            TimerPacketInner::EpochTime { current_epoch } => unsafe {
                                crate::state::EPOCH_BASE = current_epoch - Instant::now().as_secs();
                            },
                            TimerPacketInner::DelegateResponse(_) => {
                                tagged_publisher.publish((69420, timer_packet)).await;
                            }
                            TimerPacketInner::StartUpdate {
                                version: _,
                                build_time: _,
                                size,
                                crc,
                                firmware: _,
                            } => {
                                log::info!("Begin update size: {size} crc: {crc}");
                                ota.ota_begin(size, crc).map_err(|_| ())?;
                                unsafe {
                                    crate::state::OTA_STATE = true;
                                }

                                let mut state = global_state.state.lock().await;
                                state.scene = Scene::Update;
                                drop(state);

                                FRAME_CHANNEL
                                    .send(WsFrameOwned::Binary(alloc::vec::Vec::new()))
                                    .await;
                            }
                            //TimerPacket::SolveConfirm { esp_id, competitor_id, session_id } => todo!(),
                            _ => {}
                        }
                    }
                    Err(e) => {
                        log::error!("timer_packet_fail: {e:?}\nTried to parse:\n{text}\n\n");
                    }
                },
                WsFrame::Binary(data) => {
                    if !crate::state::get_ota_state() {
                        continue;
                    }

                    let res = ota.ota_write_chunk(data);
                    if res == Ok(true) {
                        log::info!("OTA complete! Veryfying..");
                        if ota.ota_flush(true, true).is_ok() {
                            log::info!("OTA restart!");
                            esp_hal::reset::software_reset();
                        } else {
                            log::error!("OTA flash verify failed!");
                        }
                    }

                    let progress = (ota.get_ota_progress() * 100.0) as u8;
                    global_state.update_progress.signal(progress);

                    FRAME_CHANNEL
                        .send(WsFrameOwned::Binary(alloc::vec::Vec::new()))
                        .await;
                }
                WsFrame::Close(_, _) => todo!(),
                WsFrame::Ping(_) => {
                    FRAME_CHANNEL
                        .send(WsFrameOwned::Pong(alloc::vec::Vec::new()))
                        .await;
                }
                _ => {}
            }
        }
    }
}

async fn ws_writer(writer: &mut TcpWriter<'_>, framer: &mut WsTxFramer<'_>) -> Result<(), ()> {
    let recv = FRAME_CHANNEL.receiver();
    loop {
        let frame = recv.receive().await;
        let data = framer.frame(frame.into_ref());
        writer.write_all(data).await.map_err(|_| ())?;
    }
}

pub async fn send_packet(packet: TimerPacket) {
    FRAME_CHANNEL
        .send(WsFrameOwned::Text(serde_json::to_string(&packet).unwrap()))
        .await;
}

#[allow(dead_code)]
pub fn clear_frame_channel() {
    FRAME_CHANNEL.clear();
}

pub async fn send_request<T>(packet: TimerPacketInner) -> Result<T, ApiError>
where
    T: FromPacket,
{
    let mut tag_bytes = [0; 8];
    _ = getrandom::getrandom(&mut tag_bytes);
    let tag = u64::from_be_bytes(tag_bytes);

    send_tagged_request(tag, packet).await
}

pub async fn send_tagged_request<T>(tag: u64, packet: TimerPacketInner) -> Result<T, ApiError>
where
    T: FromPacket,
{
    let packet = TimerPacket {
        tag: Some(tag),
        data: packet,
    };
    send_packet(packet).await;

    // TODO: timeout
    let packet = wait_for_tagged_response(tag).await;
    FromPacket::from_packet(packet)
}

async fn wait_for_tagged_response(tag: u64) -> TimerPacket {
    let mut subscriber = TAGGED_RETURN.subscriber().unwrap();
    loop {
        let (packet_tag, packet) = subscriber.next_message_pure().await;
        if packet_tag == tag {
            return packet;
        }
    }
}
