use crate::{
    consts::WS_RETRY_MS,
    state::{GlobalState, Scene},
    structs::{ApiError, FromPacket, TimerPacket, TimerPacketInner},
};
use alloc::string::String;
use core::str::FromStr;
use embassy_net::{tcp::TcpSocket, IpAddress, Stack};
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
pub async fn ws_task(stack: Stack<'static>, ws_url: String, global_state: GlobalState) {
    let ws_url = WsUrl::from_str(&ws_url).expect("Ws url parse error");

    let mut rx_buf = [0; 8192];
    let mut tx_buf = [0; 8192];
    let mut ws_rx_buf = alloc::vec![0; 8192];
    let mut ws_tx_buf = alloc::vec![0; 8192];

    // tls buffers
    let mut ssl_rx_buf = alloc::vec::Vec::new();
    let mut ssl_tx_buf = alloc::vec::Vec::new();

    #[cfg(feature = "esp32c3")]
    if ws_url.secure {
        ssl_rx_buf.resize(16640, 0);
        ssl_tx_buf.resize(16640, 0);
    }

    loop {
        let res = ws_loop(
            &global_state,
            &ws_url,
            stack,
            &mut rx_buf,
            &mut tx_buf,
            &mut ws_rx_buf,
            &mut ws_tx_buf,
            &mut ssl_rx_buf,
            &mut ssl_tx_buf,
        )
        .await;

        if let Err(e) = res {
            log::error!("Ws_loop errored! {e:?}");
        }

        Timer::after_millis(500).await;
    }
}

// TODO: maybe make less args?
#[allow(clippy::too_many_arguments)]
async fn ws_loop(
    global_state: &GlobalState,
    ws_url: &WsUrl<'_>,
    stack: Stack<'static>,
    rx_buf: &mut [u8],
    tx_buf: &mut [u8],
    ws_rx_buf: &mut [u8],
    ws_tx_buf: &mut [u8],
    ssl_rx_buf: &mut [u8],
    ssl_tx_buf: &mut [u8],
) -> Result<(), ()> {
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

            let Ok(res) = res else {
                log::error!(
                    "[WS]Dns resolver error: {:?}",
                    res.expect_err("Shouldnt fail")
                );
                Timer::after_millis(1000).await;
                continue;
            };

            let Some(IpAddress::Ipv4(addr)) = res.first() else {
                log::error!("[WS]Dns resolver empty vec");
                Timer::after_millis(1000).await;
                continue;
            };
            *addr
        };

        let mut socket = TcpSocket::new(stack, rx_buf, tx_buf);
        socket.set_timeout(Some(embassy_time::Duration::from_secs(15)));

        let remote_endpoint = (ip, ws_url.port);
        let r = socket.connect(remote_endpoint).await;
        if let Err(e) = r {
            log::error!("connect error: {:?}", e);
            Timer::after_millis(WS_RETRY_MS).await;
            continue;
        }

        let mut socket = if ws_url.secure {
            let mut tls = TlsConnection::new(socket, ssl_rx_buf, ssl_tx_buf);

            let config: TlsConfig<'_, Aes128GcmSha256> =
                TlsConfig::new().with_server_name(ws_url.host);
            tls.open::<OsRng, NoVerify>(TlsContext::new(&config, &mut OsRng))
                .await
                .map_err(|_| ())?;

            WsSocket::Tls(tls)
        } else {
            WsSocket::Raw(socket)
        };

        {
            global_state.state.lock().await.server_connected = Some(true);
        }

        log::info!("connected!");
        let mut tx_framer = WsTxFramer::new(true, ws_tx_buf);
        let mut rx_framer = WsRxFramer::new(ws_rx_buf);

        let path = alloc::format!(
            "{}?id={}&ver={}&chip={}&firmware={}",
            ws_url.path,
            crate::utils::get_efuse_u32(),
            crate::version::VERSION,
            crate::version::CHIP,
            crate::version::FIRMWARE,
        );

        socket
            .write_all(tx_framer.generate_http_upgrade(ws_url.host, &path, None))
            .await
            .map_err(|_| ())?;

        loop {
            let n = socket.read(rx_framer.mut_buf()).await.map_err(|_| ())?;
            if n == 0 {
                log::error!("error while reading http response");
                return Err(());
            }

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
                &mut socket,
            )
            .await;

            if let Err(e) = res {
                log::error!("ws_rw_error: {e:?}");
                Timer::after_millis(WS_RETRY_MS).await;
                break;
            }
        }
    }
}

async fn ws_rw(
    framer_rx: &mut WsRxFramer<'_>,
    framer_tx: &mut WsTxFramer<'_>,
    global_state: GlobalState,
    tls: &mut WsSocket<'_, '_>,
) -> Result<(), ()> {
    let mut ota = Ota::new(FlashStorage::new()).map_err(|_| ())?;
    let tagged_publisher = TAGGED_RETURN.publisher().map_err(|_| ())?;
    let recv = FRAME_CHANNEL.receiver();

    loop {
        let read_fut = tls.read(framer_rx.mut_buf());
        let write_fut = recv.receive();

        let n = match embassy_futures::select::select(read_fut, write_fut).await {
            embassy_futures::select::Either::First(read_res) => read_res,
            embassy_futures::select::Either::Second(write_frame) => {
                let data = framer_tx.frame(write_frame.into_ref());
                tls.write_all(data).await.map_err(|_| ())?;

                continue;
            }
        }?;

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
                            TimerPacketInner::DeviceSettings { added } => {
                                let mut state = global_state.state.lock().await;
                                state.device_added = Some(added);
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
                    if !crate::state::ota_state() {
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

enum WsSocket<'a, 'b> {
    Tls(TlsConnection<'b, TcpSocket<'a>, Aes128GcmSha256>),
    Raw(TcpSocket<'a>),
}

impl WsSocket<'_, '_> {
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        match self {
            WsSocket::Tls(tls_connection) => tls_connection.read(buf).await.map_err(|_| ()),
            WsSocket::Raw(tcp_socket) => tcp_socket.read(buf).await.map_err(|_| ()),
        }
    }

    pub async fn write_all(&mut self, buf: &[u8]) -> Result<(), ()> {
        match self {
            WsSocket::Tls(tls_connection) => {
                tls_connection.write_all(buf).await.map_err(|_| ())?;
                tls_connection.flush().await.map_err(|_| ())?;
            }
            WsSocket::Raw(tcp_socket) => {
                tcp_socket.write_all(buf).await.map_err(|_| ())?;
            }
        }

        Ok(())
    }
}
