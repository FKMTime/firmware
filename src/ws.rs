use crate::{
    consts::WS_RETRY_MS,
    state::{GlobalState, Scene, ota_state},
    structs::{ApiError, FromPacket, TimerPacket, TimerPacketInner},
};
use alloc::{boxed::Box, rc::Rc, string::ToString};
use core::str::FromStr;
use embassy_net::{IpAddress, Stack, tcp::TcpSocket};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, pubsub::PubSubChannel,
    signal::Signal,
};
use embassy_time::{Duration, Instant, Timer, WithTimeout};
use embedded_io_async::Write;
use embedded_tls::{Aes128GcmSha256, NoVerify, TlsConfig, TlsConnection, TlsContext};
use esp_hal_ota::Ota;
use esp_storage::FlashStorage;
use rand_core::OsRng;
use ws_framer::{WsFrame, WsFrameOwned, WsRxFramer, WsTxFramer, WsUrl, WsUrlOwned};

static FRAME_CHANNEL: Channel<CriticalSectionRawMutex, WsFrameOwned, 10> = Channel::new();
static TAGGED_RETURN: PubSubChannel<CriticalSectionRawMutex, (u64, TimerPacket), 20, 20, 4> =
    PubSubChannel::new();

const WS_BUF_SIZE: usize = 8192;
const TLS_BUF_SIZE: usize = 16640;

#[embassy_executor::task]
pub async fn ws_task(
    stack: Stack<'static>,
    ws_url: WsUrlOwned,
    global_state: GlobalState,
    ws_sleep_sig: Rc<Signal<CriticalSectionRawMutex, bool>>,
) {
    log::debug!("ws_url: {ws_url:?}");

    let mut rx_buf = [0; WS_BUF_SIZE];
    let mut tx_buf = [0; WS_BUF_SIZE];
    let mut ws_rx_buf = alloc::vec![0; WS_BUF_SIZE];
    let mut ws_tx_buf = alloc::vec![0; WS_BUF_SIZE];

    // tls buffers
    let mut ssl_rx_buf = alloc::vec::Vec::new();
    let mut ssl_tx_buf = alloc::vec::Vec::new();

    if ws_url.secure {
        ssl_rx_buf.resize(TLS_BUF_SIZE, 0);
        ssl_tx_buf.resize(TLS_BUF_SIZE, 0);
    }

    loop {
        let ws_fut = ws_loop(
            &global_state,
            ws_url.as_ref(),
            stack,
            &mut rx_buf,
            &mut tx_buf,
            &mut ws_rx_buf,
            &mut ws_tx_buf,
            &mut ssl_rx_buf,
            &mut ssl_tx_buf,
        );

        let res = embassy_futures::select::select(ws_fut, ws_sleep_sig.wait()).await;

        match res {
            embassy_futures::select::Either::First(res) => {
                if let Err(e) = res {
                    log::error!("Ws_loop errored! {e:?}");
                }
            }
            embassy_futures::select::Either::Second(sleep) => {
                if sleep {
                    loop {
                        let sleep = ws_sleep_sig.wait().await;
                        if !sleep {
                            break;
                        }
                    }
                }
            }
        }

        Timer::after_millis(500).await;
    }
}

// TODO: maybe make less args?
#[allow(clippy::too_many_arguments)]
async fn ws_loop(
    global_state: &GlobalState,
    ws_url: WsUrl<'_>,
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
                log::error!("[WS]Dns resolver error: {:?}", res.expect_err(""));
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
            log::error!("connect error: {e:?}");
            Timer::after_millis(WS_RETRY_MS).await;
            continue;
        }

        let mut socket = if ws_url.secure {
            let mut tls = TlsConnection::new(socket, ssl_rx_buf, ssl_tx_buf);

            let config: TlsConfig<'_, Aes128GcmSha256> = TlsConfig::new().enable_rsa_signatures();
            tls.open::<OsRng, NoVerify>(TlsContext::new(&config, &mut OsRng))
                .await
                .map_err(|e| {
                    log::error!("tls open error: {e:?}");
                })?;

            WsSocket::Tls(Box::new(tls))
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
            "{}?id={}&ver={}&hw={}&firmware={}",
            ws_url.path,
            crate::utils::get_efuse_u32(),
            crate::version::VERSION,
            crate::version::HW_VER,
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

        #[cfg(feature = "auto_add")]
        {
            if !global_state
                .state
                .lock()
                .await
                .device_added
                .unwrap_or(false)
            {
                crate::ws::send_packet(crate::structs::TimerPacket {
                    tag: None,
                    data: crate::structs::TimerPacketInner::Add {
                        firmware: alloc::string::ToString::to_string(crate::version::FIRMWARE),
                        sign_key: unsafe { crate::state::SIGN_KEY },
                    },
                    sign_key: None,
                })
                .await;
            }
        }

        loop {
            let res = ws_rw(
                &mut rx_framer,
                &mut tx_framer,
                global_state.clone(),
                &mut socket,
            )
            .await;

            if let Err(e) = res {
                if ota_state() {
                    global_state.state.lock().await.custom_message =
                        Some(("Connection lost".to_string(), "during update".to_string()));

                    Timer::after_millis(5000).await;
                    esp_hal::system::software_reset();
                }

                log::error!("ws_rw_error: {e:?}");
                Timer::after_millis(WS_RETRY_MS).await;
                break;
            }
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
enum WsRwError {
    OtaError(esp_hal_ota::OtaError),
    TaggedPublisherError,
    SocketWriteError,
    SocketReadError,
    Other,
}

async fn ws_rw(
    framer_rx: &mut WsRxFramer<'_>,
    framer_tx: &mut WsTxFramer<'_>,
    global_state: GlobalState,
    socket: &mut WsSocket<'_, '_>,
) -> Result<(), WsRwError> {
    let mut ota = Ota::new(FlashStorage::new()).map_err(WsRwError::OtaError)?;
    let tagged_publisher = TAGGED_RETURN
        .publisher()
        .map_err(|_| WsRwError::TaggedPublisherError)?;
    let recv = FRAME_CHANNEL.receiver();

    loop {
        let read_fut = socket.read(framer_rx.mut_buf());
        let write_fut = recv.receive();

        let Ok(n) = (match embassy_futures::select::select(read_fut, write_fut).await {
            embassy_futures::select::Either::First(read_res) => {
                read_res.map_err(|_| WsRwError::SocketReadError)
            }
            embassy_futures::select::Either::Second(write_frame) => {
                let mut offset = 0;
                let frame_ref = write_frame.into_ref();

                loop {
                    let (data, finish) = framer_tx.partial_frame(&frame_ref, &mut offset);
                    socket
                        .write_all(data)
                        .await
                        .map_err(|_| WsRwError::SocketWriteError)?;
                    if !finish {
                        break;
                    }

                    log::warn!("Frame splitted!");
                }

                continue;
            }
        }) else {
            continue;
        };

        if n == 0 {
            log::warn!("read_n: 0");
            return Err(WsRwError::Other);
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
                                added,
                                locales,
                                default_locale,
                            } => {
                                let mut state = global_state.state.lock().await;
                                state.device_added = Some(added);
                                crate::translations::clear_locales();

                                for locale in locales {
                                    crate::translations::process_locale(
                                        locale.locale,
                                        locale.translations,
                                    );
                                }

                                crate::translations::select_locale(&default_locale, &global_state);
                                crate::translations::set_default_locale();
                            }
                            TimerPacketInner::ApiError(e) => {
                                // if should_reset_time reset time
                                let mut state = global_state.state.lock().await;
                                state.error_text = Some(e.error);
                            }
                            TimerPacketInner::CustomMessage { line1, line2 } => {
                                let mut state = global_state.state.lock().await;
                                state.custom_message = Some((line1, line2));
                            }
                            TimerPacketInner::EpochTime { current_epoch } => unsafe {
                                crate::state::EPOCH_BASE = current_epoch - Instant::now().as_secs();
                            },
                            TimerPacketInner::DelegateResponse(_) => {
                                tagged_publisher.publish((69420, timer_packet)).await;
                            }
                            TimerPacketInner::StartUpdate {
                                version,
                                build_time: _,
                                size,
                                crc,
                                firmware,
                            } => {
                                if firmware != crate::version::FIRMWARE {
                                    continue;
                                }

                                log::info!("Start update: {firmware}/{version}");
                                log::info!("Begin update size: {size} crc: {crc}");
                                ota.ota_begin(size, crc).map_err(WsRwError::OtaError)?;
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

                            #[cfg(feature = "e2e")]
                            TimerPacketInner::TestPacket(test_packet) => {
                                parse_test_packet(test_packet, &global_state).await;
                            }
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
                            esp_hal::system::software_reset();
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

#[cfg(feature = "e2e")]
async fn parse_test_packet(
    test_packet: crate::structs::TestPacketData,
    global_state: &GlobalState,
) {
    log::warn!("TEST PACKET: {test_packet:?}");

    match test_packet {
        crate::structs::TestPacketData::ResetState => {
            global_state
                .state
                .lock()
                .await
                .reset_solve_state(None)
                .await;

            global_state
                .e2e
                .stackmat_sig
                .signal((crate::utils::stackmat::StackmatTimerState::Reset, 0));

            send_test_ack(&global_state).await;
        }
        crate::structs::TestPacketData::HardStateReset => {
            global_state.state.lock().await.hard_state_reset().await;
        }
        crate::structs::TestPacketData::ScanCard(uid) => {
            global_state.e2e.card_scan_sig.signal(uid as u128)
        }
        crate::structs::TestPacketData::ButtonPress { pin, press_time } => global_state
            .e2e
            .buttons_sig
            .signal((pin as usize, press_time)),
        crate::structs::TestPacketData::StackmatTime(ms) => global_state
            .e2e
            .stackmat_sig
            .signal((crate::utils::stackmat::StackmatTimerState::Running, ms)),
        crate::structs::TestPacketData::StackmatReset => {
            global_state
                .e2e
                .stackmat_sig
                .signal((crate::utils::stackmat::StackmatTimerState::Reset, 0));
        }
    }
}

#[cfg(feature = "e2e")]
pub async fn send_test_ack(global_state: &GlobalState) {
    send_packet(TimerPacket {
        tag: None,
        data: TimerPacketInner::TestAck(global_state.state.value().await.snapshot_data()),
        sign_key: Some(unsafe { crate::state::SIGN_KEY }),
    })
    .await;
}

pub async fn send_packet(packet: TimerPacket) {
    match serde_json::to_string(&packet) {
        Ok(string) => {
            FRAME_CHANNEL.send(WsFrameOwned::Text(string)).await;
        }
        Err(e) => {
            log::error!("send_packet json to_string failed: {e:?}");
        }
    }
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

    send_tagged_request(tag, packet, true).await
}

pub async fn send_tagged_request<T>(
    tag: u64,
    packet: TimerPacketInner,
    timeout: bool,
) -> Result<T, ApiError>
where
    T: FromPacket,
{
    let packet = TimerPacket {
        tag: Some(tag),
        data: packet,
        sign_key: Some(unsafe { crate::state::SIGN_KEY }),
    };
    send_packet(packet).await;

    let packet = if timeout {
        wait_for_tagged_response(tag)
            .with_timeout(Duration::from_millis(5000))
            .await
            .map_err(|_| ApiError {
                should_reset_time: false,
                error: "Communication timeout!".to_string(),
            })?
    } else {
        wait_for_tagged_response(tag).await
    };

    FromPacket::from_packet(packet)
}

async fn wait_for_tagged_response(tag: u64) -> TimerPacket {
    loop {
        match TAGGED_RETURN.subscriber() {
            Ok(mut subscriber) => loop {
                let (packet_tag, packet) = subscriber.next_message_pure().await;
                if packet_tag == tag {
                    return packet;
                }
            },
            Err(_) => {
                log::error!("failed to get TAGGED_RETURN subscriber! Retry!");
                Timer::after_millis(500).await;
            }
        }
    }
}

enum WsSocket<'a, 'b> {
    Tls(Box<TlsConnection<'b, TcpSocket<'a>, Aes128GcmSha256>>),
    Raw(TcpSocket<'a>),
}

impl WsSocket<'_, '_> {
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        match self {
            WsSocket::Tls(tls_connection) => tls_connection.read(buf).await.map_err(|e| {
                log::error!("tls read error: {e:?}");
            }),
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
