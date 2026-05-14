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
use embedded_tls::{Aes128GcmSha256, NoVerify, TlsConfig, TlsConnection, TlsContext};
use esp_hal_ota::Ota;
use esp_storage::FlashStorage;
use rand_core::OsRng;
use ws_framer::{WsFrame, WsFrameOwned, WsRxFramer, WsTxFramer, WsUrl, WsUrlOwned};

static FRAME_CHANNEL: Channel<CriticalSectionRawMutex, WsFrameOwned, 32> = Channel::new();
static TAGGED_RETURN: PubSubChannel<CriticalSectionRawMutex, (u64, TimerPacket), 20, 20, 4> =
    PubSubChannel::new();

static DNS_EMPTY_LOGGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static HTTP_UPGRADE_LOGGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static PACKET_PARSE_LOGGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static PACKET_SERIALIZE_LOGGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static TAGGED_SUBSCRIBER_LOGGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

const WS_BUF_SIZE: usize = 8192;
const TLS_BUF_SIZE: usize = 16640;

#[unsafe(link_section = ".dram2_uninit")]
static mut WS_RX_BUF: core::mem::MaybeUninit<[u8; WS_BUF_SIZE]> = core::mem::MaybeUninit::uninit();
#[unsafe(link_section = ".dram2_uninit")]
static mut WS_TX_BUF: core::mem::MaybeUninit<[u8; WS_BUF_SIZE]> = core::mem::MaybeUninit::uninit();
#[unsafe(link_section = ".dram2_uninit")]
static mut TLS_RX_BUF: core::mem::MaybeUninit<[u8; TLS_BUF_SIZE]> =
    core::mem::MaybeUninit::uninit();
#[unsafe(link_section = ".dram2_uninit")]
static mut TLS_TX_BUF: core::mem::MaybeUninit<[u8; TLS_BUF_SIZE]> =
    core::mem::MaybeUninit::uninit();

#[embassy_executor::task]
pub async fn ws_task(
    stack: Stack<'static>,
    ws_url: WsUrlOwned,
    global_state: GlobalState,
    ws_sleep_sig: Rc<Signal<CriticalSectionRawMutex, bool>>,
    wifi_conn_sig: Rc<Signal<CriticalSectionRawMutex, bool>>,
) {
    log::debug!("ws_url: {ws_url:?}");

    let mut rx_buf = [0; WS_BUF_SIZE];
    let mut tx_buf = [0; WS_BUF_SIZE];

    #[allow(static_mut_refs)]
    let (ws_rx_buf, ws_tx_buf, ssl_rx_buf, ssl_tx_buf): (
        &mut [u8],
        &mut [u8],
        &mut [u8],
        &mut [u8],
    ) = unsafe {
        (
            &mut *WS_RX_BUF.as_mut_ptr(),
            &mut *WS_TX_BUF.as_mut_ptr(),
            &mut *TLS_RX_BUF.as_mut_ptr(),
            &mut *TLS_TX_BUF.as_mut_ptr(),
        )
    };

    loop {
        unsafe { crate::state::TRUST_SERVER = false };
        unsafe { crate::state::SECURE_RFID = false };
        unsafe { crate::state::AUTO_SETUP = false };
        unsafe { crate::state::FKM_TOKEN = 0 };

        let ws_fut = ws_loop(
            &global_state,
            ws_url.as_ref(),
            stack,
            &mut rx_buf,
            &mut tx_buf,
            &mut *ws_rx_buf,
            &mut *ws_tx_buf,
            &mut *ssl_rx_buf,
            &mut *ssl_tx_buf,
            &wifi_conn_sig,
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
    wifi_conn_sig: &Rc<Signal<CriticalSectionRawMutex, bool>>,
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

            let res = match res {
                Ok(res) => res,
                Err(e) => {
                    log::error!("[WS]Dns resolver error: {e:?}");
                    Timer::after_millis(1000).await;
                    continue;
                }
            };

            let Some(IpAddress::Ipv4(addr)) = res.first() else {
                log::error!("[WS]Dns resolver empty vec");
                if !DNS_EMPTY_LOGGED.load(core::sync::atomic::Ordering::Relaxed) {
                    crate::utils::error_log::add_error(
                        crate::utils::error_log::codes::WS_DNS_RESOLVE_EMPTY,
                    )
                    .await;

                    DNS_EMPTY_LOGGED.store(true, core::sync::atomic::Ordering::Relaxed);
                }
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
            // but if wifi conneceted signal was sent remove wifi connection lost msg
            if wifi_conn_sig.signaled() && wifi_conn_sig.wait().await {
                global_state.state.lock().await.wifi_connected = Some(true);
            }

            log::error!("connect error: {e:?}");
            Timer::after_millis(WS_RETRY_MS).await;
            continue;
        }

        let mut socket = if ws_url.secure {
            let mut tls = TlsConnection::new(socket, ssl_rx_buf, ssl_tx_buf);

            let config = TlsConfig::new().enable_rsa_signatures();
            tls.open(TlsContext::new(
                &config,
                Provider {
                    rng: OsRng,
                    verifier: NoVerify {},
                },
            ))
            .await
            .map_err(|e| {
                log::error!("tls open error: {e:?}");
            })?;

            WsSocket::Tls(Box::new(tls))
        } else {
            WsSocket::Raw(socket)
        };

        {
            let mut state = global_state.state.lock().await;
            state.server_connected = Some(true);
            state.wifi_connected = Some(true);
        }

        log::info!("connected!");
        let mut tx_framer = WsTxFramer::new(true, ws_tx_buf);
        let mut rx_framer = WsRxFramer::new(ws_rx_buf);

        let random = crate::utils::get_random_u64();
        let path = alloc::format!(
            "{}?id={}&ver={}&hw={}&firmware={}&random={}",
            ws_url.path,
            crate::utils::get_efuse_u32(),
            crate::version::VERSION,
            crate::version::HW_VER,
            crate::version::FIRMWARE,
            random
        );

        socket
            .write_all(tx_framer.generate_http_upgrade(ws_url.host, &path, None))
            .await
            .map_err(|_| ())?;

        let headers = loop {
            let n = socket.read(rx_framer.mut_buf()).await.map_err(|_| ())?;
            if n == 0 {
                log::error!("error while reading http response");
                if !HTTP_UPGRADE_LOGGED.load(core::sync::atomic::Ordering::Relaxed) {
                    crate::utils::error_log::add_error(
                        crate::utils::error_log::codes::WS_HTTP_UPGRADE_READ_FAILED,
                    )
                    .await;

                    HTTP_UPGRADE_LOGGED.store(true, core::sync::atomic::Ordering::Relaxed);
                }
                return Err(());
            }

            let res = rx_framer.process_http_response(n);
            if let Some(resp) = res {
                log::info!("http_resp_code: {}", resp.status_code);
                break resp.headers;
            }
        };

        if let Some(Ok(random_signed)) = headers
            .iter()
            .find(|h| h.name.to_lowercase() == "randomsigned")
            .map(|h| h.value.parse::<u128>())
        {
            let mut key = [0; 16];
            key[..4].copy_from_slice(&unsafe { crate::state::SIGN_KEY.to_be_bytes() });

            let mut block = [0; 16];
            block.copy_from_slice(&random_signed.to_be_bytes());

            global_state
                .aes
                .lock()
                .await
                .decrypt(&mut block, esp_hal::aes::Key::Key128(key));

            let recv_random = u64::from_be_bytes(block[..8].try_into().unwrap_or_default());
            let fkm_token = i32::from_be_bytes(block[8..12].try_into().unwrap_or_default());

            log::debug!(
                "[trust] random: {random}, recv_random: {recv_random} | fkm_token: {fkm_token}"
            );
            if random == recv_random {
                unsafe { crate::state::TRUST_SERVER = true };
                unsafe { crate::state::FKM_TOKEN = fkm_token };
            } else {
                #[cfg(not(feature = "e2e"))]
                {
                    global_state.state.lock().await.error_text =
                        Some("Server Not Trusted!".to_string());
                }
            }
        }

        _ = FRAME_CHANNEL.try_send(WsFrameOwned::Ping(alloc::vec::Vec::new()));

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
                wifi_conn_sig,
            )
            .await;

            if let Err(e) = res {
                if ota_state() {
                    log::error!("Connection lost during OTA update: {e:?}");
                    crate::utils::error_log::add_error(
                        crate::utils::error_log::codes::WS_CONNECTION_LOST_DURING_OTA,
                    )
                    .await;
                    crate::utils::error_log::save_error_log(&global_state.nvs).await;

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
    WifiDisconnected,
    Other,
}

async fn ws_rw(
    framer_rx: &mut WsRxFramer<'_>,
    framer_tx: &mut WsTxFramer<'_>,
    global_state: GlobalState,
    socket: &mut WsSocket<'_, '_>,
    wifi_conn_sig: &Rc<Signal<CriticalSectionRawMutex, bool>>,
) -> Result<(), WsRwError> {
    let mut ota = Ota::new(FlashStorage::new(unsafe {
        esp_hal::peripherals::FLASH::steal()
    }))
    .map_err(WsRwError::OtaError)?;
    let tagged_publisher = TAGGED_RETURN
        .publisher()
        .map_err(|_| WsRwError::TaggedPublisherError)?;
    let recv = FRAME_CHANNEL.receiver();

    loop {
        let read_fut = socket.read(framer_rx.mut_buf());
        let write_fut = recv.receive();

        let res = match embassy_futures::select::select3(read_fut, write_fut, wifi_conn_sig.wait())
            .await
        {
            embassy_futures::select::Either3::First(read_res) => {
                read_res.map_err(|_| WsRwError::SocketReadError)
            }
            embassy_futures::select::Either3::Second(write_frame) => {
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
            embassy_futures::select::Either3::Third(state) => {
                if state {
                    // wifi connected signal
                    continue;
                }

                global_state.state.lock().await.wifi_connected = Some(false);
                log::error!("Wifi disconnected, ws_rw stop.");
                return Err(WsRwError::WifiDisconnected);
            }
        };

        let n = match res {
            Ok(n) => n,
            Err(e) => return Err(e),
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
                                fkm_token,
                                secure_rfid,
                                auto_setup,
                                sound_enabled,
                            } => {
                                let mut state = global_state.state.lock().await;
                                state.device_added = Some(added);
                                state.sound_enabled = sound_enabled;
                                crate::translations::clear_locales();

                                for locale in locales {
                                    crate::translations::process_locale(
                                        locale.locale,
                                        locale.translations,
                                    );
                                }

                                crate::translations::select_locale(&default_locale, &global_state);
                                crate::translations::set_default_locale();

                                unsafe { crate::state::FKM_TOKEN = fkm_token };
                                unsafe { crate::state::SECURE_RFID = secure_rfid };
                                unsafe { crate::state::AUTO_SETUP = auto_setup };
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

                                #[cfg(not(feature = "e2e"))]
                                if unsafe { !crate::state::TRUST_SERVER } {
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

                                clear_frame_channel();
                                FRAME_CHANNEL
                                    .send(WsFrameOwned::Binary(alloc::vec::Vec::new()))
                                    .await;
                            }

                            #[allow(clippy::collapsible_match)]
                            TimerPacketInner::SetDeviceSettings { volume } => {
                                #[cfg(feature = "v4")]
                                if let Some(volume) = volume {
                                    let volume = volume.clamp(
                                        crate::consts::BUZZER_VOLUME_MIN,
                                        crate::consts::BUZZER_VOLUME_MAX,
                                    );
                                    crate::state::set_buzzer_volume(volume);

                                    if let Err(e) = global_state
                                        .nvs
                                        .set(crate::consts::NVS_BUZZER_VOLUME, volume)
                                        .await
                                    {
                                        log::error!("Cannot save buzzer volume to NVS: {e:?}");
                                        static LOGGED: core::sync::atomic::AtomicBool =
                                            core::sync::atomic::AtomicBool::new(false);
                                        if !LOGGED.load(core::sync::atomic::Ordering::Relaxed) {
                                            crate::utils::error_log::add_error(
                        crate::utils::error_log::codes::NVS_BUZZER_VOLUME_WRITE_FAILED,
                    )
                    .await;

                                            LOGGED
                                                .store(true, core::sync::atomic::Ordering::Relaxed);
                                        }
                                    }
                                }

                                #[cfg(feature = "v3")]
                                {
                                    _ = volume;
                                }
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
                        if !PACKET_PARSE_LOGGED.load(core::sync::atomic::Ordering::Relaxed) {
                            crate::utils::error_log::add_error(
                                crate::utils::error_log::codes::WS_PACKET_PARSE_FAILED,
                            )
                            .await;

                            PACKET_PARSE_LOGGED.store(true, core::sync::atomic::Ordering::Relaxed);
                        }
                    }
                },
                WsFrame::Binary(data) => {
                    if !crate::state::ota_state() {
                        continue;
                    }

                    #[cfg(not(feature = "e2e"))]
                    if unsafe { !crate::state::TRUST_SERVER } {
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
                            crate::utils::error_log::add_error(
                                crate::utils::error_log::codes::OTA_VERIFY_FAILED,
                            )
                            .await;
                        }
                    }

                    let progress = (ota.get_ota_progress() * 100.0) as u8;
                    global_state.update_progress.signal(progress);

                    FRAME_CHANNEL
                        .send(WsFrameOwned::Binary(alloc::vec::Vec::new()))
                        .await;
                }
                WsFrame::Ping(_) => {
                    _ = FRAME_CHANNEL.try_send(WsFrameOwned::Pong(alloc::vec::Vec::new()));
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
            global_state
                .e2e
                .stackmat_sig
                .signal((crate::utils::stackmat::StackmatTimerState::Reset, 0));

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
            if !PACKET_SERIALIZE_LOGGED.load(core::sync::atomic::Ordering::Relaxed) {
                crate::utils::error_log::add_error(
                    crate::utils::error_log::codes::WS_PACKET_SERIALIZE_FAILED,
                )
                .await;

                PACKET_SERIALIZE_LOGGED.store(true, core::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

pub async fn send_frame(frame: WsFrameOwned) {
    FRAME_CHANNEL.send(frame).await;
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
    };

    send_packet(packet)
        .with_timeout(Duration::from_millis(5000))
        .await
        .map_err(|_| ApiError {
            should_reset_time: false,
            error: "Channel full".to_string(),
        })?;

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
                if !TAGGED_SUBSCRIBER_LOGGED.load(core::sync::atomic::Ordering::Relaxed) {
                    crate::utils::error_log::add_error(
                        crate::utils::error_log::codes::WS_TAGGED_SUBSCRIBER_FAILED,
                    )
                    .await;

                    TAGGED_SUBSCRIBER_LOGGED.store(true, core::sync::atomic::Ordering::Relaxed);
                }
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
            WsSocket::Tls(tls_connection) => tls_connection.read(buf).await.map_err(|_| ()),
            WsSocket::Raw(tcp_socket) => tcp_socket.read(buf).await.map_err(|_| ()),
        }
    }

    pub async fn write_all(&mut self, buf: &[u8]) -> Result<(), ()> {
        let mut written = 0;
        while written < buf.len() {
            written += self.write(&buf[written..]).await?;
        }

        Ok(())
    }

    pub async fn write(&mut self, buf: &[u8]) -> Result<usize, ()> {
        let n = match self {
            WsSocket::Tls(tls_connection) => {
                let n = tls_connection.write(buf).await.map_err(|_| ())?;
                tls_connection.flush().await.map_err(|_| ())?;
                n
            }
            WsSocket::Raw(tcp_socket) => tcp_socket.write(buf).await.map_err(|_| ())?,
        };

        Ok(n)
    }
}

struct Provider {
    rng: OsRng,
    verifier: NoVerify,
}

impl embedded_tls::CryptoProvider for Provider {
    type CipherSuite = Aes128GcmSha256;

    type Signature = &'static [u8];

    fn rng(&mut self) -> impl embedded_tls::CryptoRngCore {
        &mut self.rng
    }

    fn verifier(
        &mut self,
    ) -> Result<&mut impl embedded_tls::TlsVerifier<Self::CipherSuite>, embedded_tls::TlsError>
    {
        Ok(&mut self.verifier)
    }
}
