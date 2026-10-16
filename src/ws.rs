//! Websocket connection to the timer server.
//!
//! One task owns the socket. Other tasks queue packets ([`send`], [`request`]) and wake it;
//! the task serializes them straight into the tx buffer. The reader never waits on that queue:
//! replies it owes (pong, OTA ack, crash log) are flags the writer serves first.

use crate::{
    consts::WS_RETRY_MS,
    state::{GlobalState, Scene},
    structs::{ApiError, DelegateResponsePacket, FromPacket, TimerPacket, TimerPacketInner},
    utils::{UninitBuf, error_log::codes, logger::ReadyLogs},
};
use core::{cell::Cell, fmt::Write as _, str::FromStr};
use embassy_crypto_rustcrypto as _;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_net::{IpAddress, Stack, tcp::TcpSocket};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    channel::Channel,
    signal::Signal,
};
use embassy_time::{Duration, Timer, WithTimeout};
use embedded_tls::{Aes128GcmSha256, InsecureNoVerify, TlsConfig, TlsConnection, TlsContext};
use esp_hal_ota::Ota;
use esp_storage::FlashStorage;
use portable_atomic::{AtomicBool, AtomicU64, Ordering};
use ws_framer::{WsFrame, WsRxError, WsRxFramer, WsTxFramer, WsUrl};

const WS_BUF_SIZE: usize = 8192;
const TLS_BUF_SIZE: usize = 16640;
const OUT_QUEUE_LEN: usize = 8;
const REQUEST_SLOTS: usize = 4;
/// Close code telling the server the device goes to sleep.
const CLOSE_SLEEP: u16 = 4000;

#[unsafe(link_section = ".dram2_uninit")]
static WS_RX_BUF: UninitBuf<WS_BUF_SIZE> = UninitBuf::new();
#[unsafe(link_section = ".dram2_uninit")]
static WS_TX_BUF: UninitBuf<WS_BUF_SIZE> = UninitBuf::new();
#[unsafe(link_section = ".dram2_uninit")]
static TLS_RX_BUF: UninitBuf<TLS_BUF_SIZE> = UninitBuf::new();
#[unsafe(link_section = ".dram2_uninit")]
static TLS_TX_BUF: UninitBuf<TLS_BUF_SIZE> = UninitBuf::new();

type Cs = CriticalSectionRawMutex;

static OUT: Channel<Cs, TimerPacket, OUT_QUEUE_LEN> = Channel::new();

/// Set once an update starts; the device resets when it ends.
static OTA_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn ota_active() -> bool {
    OTA_ACTIVE.load(Ordering::Relaxed)
}
/// Wakes the connection task when something was queued for it.
static WAKE: Signal<Cs, ()> = Signal::new();
/// `true` = going to sleep (connection dropped until `false`), sent by [`crate::power::Supervisor`].
pub static SLEEP: crate::power::SleepSignal = Signal::new();
static PENDING_LOGS: Mutex<Cs, Cell<Option<ReadyLogs>>> = Mutex::new(Cell::new(None));
static PENDING_CLOSE: Mutex<Cs, Cell<Option<(u16, &'static str)>>> = Mutex::new(Cell::new(None));

/// A request waiting for the response with its tag (tag 0 = free slot).
struct Slot {
    tag: AtomicU64,
    response: Signal<Cs, TimerPacket>,
}

static SLOTS: [Slot; REQUEST_SLOTS] = [const {
    Slot {
        tag: AtomicU64::new(0),
        response: Signal::new(),
    }
}; REQUEST_SLOTS];

/// Tag of the pending delegate request: the server answers it with an untagged
/// `DelegateResponse` push, routed to that request's slot (0 = none).
static DELEGATE_TAG: AtomicU64 = AtomicU64::new(0);

/// The outgoing queue is full; nothing was sent.
#[derive(Debug)]
pub struct WsBusy;

/// Queues `packet` without waiting.
pub fn send(packet: TimerPacket) -> Result<(), WsBusy> {
    OUT.try_send(packet).map_err(|_| WsBusy)?;
    WAKE.signal(());
    Ok(())
}

/// Hands a finished log batch to the connection (sent as one binary message).
pub fn send_logs(logs: ReadyLogs) -> Result<(), WsBusy> {
    let busy = PENDING_LOGS.lock(|l| {
        let busy = l.take().is_some();
        l.set(Some(logs));
        busy
    });
    WAKE.signal(());
    if busy { Err(WsBusy) } else { Ok(()) }
}

/// Sends a Close frame (code [`CLOSE_SLEEP`] also pauses the connection for 30 s).
pub fn close(code: u16, reason: &'static str) {
    PENDING_CLOSE.lock(|c| c.set(Some((code, reason))));
    WAKE.signal(());
}

#[cfg(feature = "e2e")]
pub fn send_test_ack(global_state: &GlobalState) {
    let data = TimerPacketInner::TestAck(global_state.solve.read(|s| s.snapshot_data()));
    _ = send(TimerPacket { tag: None, data });
}

/// Frees its request slot when dropped (also when the waiting future is cancelled).
struct SlotGuard(&'static Slot);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let tag = self.0.tag.swap(0, Ordering::AcqRel);
        _ = DELEGATE_TAG.compare_exchange(tag, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
}

fn claim_slot(tag: u64) -> Result<SlotGuard, ApiError> {
    let slot = SLOTS
        .iter()
        .find(|s| {
            s.tag
                .compare_exchange(0, tag, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        })
        .ok_or_else(|| api_error("Too many requests"))?;
    slot.response.reset();
    Ok(SlotGuard(slot))
}

/// Sends a request and queues it under a fresh tag; the caller waits on the slot.
fn send_request(data: TimerPacketInner) -> Result<SlotGuard, ApiError> {
    let tag = crate::utils::get_random_u64().max(1);
    let slot = claim_slot(tag)?;
    send(TimerPacket {
        tag: Some(tag),
        data,
    })
    .map_err(|_| api_error("Channel full"))?;
    Ok(slot)
}

/// Sends `packet` and waits (at most `timeout` in total) for the tagged response.
pub async fn request<T: FromPacket>(
    packet: TimerPacketInner,
    timeout: Duration,
) -> Result<T, ApiError> {
    let slot = send_request(packet)?;
    let response = slot
        .0
        .response
        .wait()
        .with_timeout(timeout)
        .await
        .map_err(|_| timeout_error())?;
    T::from_packet(response)
}

#[cfg_attr(feature = "qa", allow(dead_code))]
/// Sends a delegate `Solve` and waits (at most `timeout`) for the server's untagged
/// `DelegateResponse` push (answered by a person, hence the long timeout). A tagged reply
/// (an `ApiError`) ends the wait early.
pub async fn delegate_request(
    packet: TimerPacketInner,
    timeout: Duration,
) -> Result<DelegateResponsePacket, ApiError> {
    let slot = send_request(packet)?;
    // no await since queueing, so the connection task can't have seen the response yet
    DELEGATE_TAG.store(slot.0.tag.load(Ordering::Acquire), Ordering::Release);
    let response = slot
        .0
        .response
        .wait()
        .with_timeout(timeout)
        .await
        .map_err(|_| timeout_error())?;
    DelegateResponsePacket::from_packet(response)
}

fn api_error(error: &str) -> ApiError {
    ApiError {
        should_reset_time: false,
        error: error.into(),
    }
}

fn timeout_error() -> ApiError {
    api_error("Communication timeout!")
}

#[embassy_executor::task]
pub async fn ws_task(
    stack: Stack<'static>,
    ws_url: heapless::String<255>,
    global_state: GlobalState,
    wifi_conn_sig: &'static Signal<Cs, bool>,
) {
    let Some(ws_url) = WsUrl::parse(&ws_url) else {
        log::error!("Invalid ws url: {ws_url}");
        return;
    };
    log::debug!("ws_url: {ws_url:?}");

    let mut tcp_rx = [0; WS_BUF_SIZE];
    let mut tcp_tx = [0; WS_BUF_SIZE];
    // SAFETY: `ws_task` is spawned once and these are the only references to the buffers
    // (each is taken, and zeroed, exactly once).
    let mut bufs = unsafe {
        Buffers {
            ws_rx: WS_RX_BUF.get(&mut false),
            ws_tx: WS_TX_BUF.get(&mut false),
            tls_rx: TLS_RX_BUF.get(&mut false),
            tls_tx: TLS_TX_BUF.get(&mut false),
            tcp_rx: &mut tcp_rx,
            tcp_tx: &mut tcp_tx,
        }
    };

    loop {
        crate::watchdog::heartbeat(crate::watchdog::TaskId::Ws);
        crate::state::SESSION.write(crate::state::Session::reset_connection);

        let conn = run(global_state, ws_url, stack, &mut bufs, wifi_conn_sig);
        match select(conn, SLEEP.wait()).await {
            Either::First(Err(e)) => {
                if ota_active() {
                    log::error!("Connection lost during OTA update: {e:?}");
                    crate::utils::error_log::add_error(codes::WS_CONNECTION_LOST_DURING_OTA);
                    crate::utils::error_log::save_error_log(global_state.nvs).await;
                    global_state.ui.update(|u| {
                        u.custom_message = Some(("Connection lost".into(), "during update".into()))
                    });

                    Timer::after_millis(5000).await;
                    esp_hal::system::software_reset();
                }
                log::error!("ws error: {e:?}");
            }
            Either::Second(true) => while SLEEP.wait().await {},
            Either::Second(false) => {}
        }

        global_state
            .status
            .update(|s| s.server_connected = Some(false));
        Timer::after_millis(WS_RETRY_MS).await;
    }
}

struct Buffers<'a> {
    ws_rx: &'a mut [u8],
    ws_tx: &'a mut [u8],
    tls_rx: &'a mut [u8],
    tls_tx: &'a mut [u8],
    tcp_rx: &'a mut [u8],
    tcp_tx: &'a mut [u8],
}

#[derive(Debug)]
#[allow(dead_code)] // fields are only read through Debug (logs)
enum WsError {
    Dns,
    Connect,
    Tls,
    Socket,
    /// Handshake rejected (status code; 101 with a wrong `Sec-WebSocket-Accept` too)
    Upgrade(u16),
    Closed,
    WifiDisconnected,
    Ota(esp_hal_ota::Error),
}

/// One connection: connect, handshake, then serve until it fails.
async fn run(
    global_state: GlobalState,
    url: WsUrl<'_>,
    stack: Stack<'static>,
    bufs: &mut Buffers<'_>,
    wifi_conn_sig: &Signal<Cs, bool>,
) -> Result<core::convert::Infallible, WsError> {
    global_state
        .status
        .update(|s| s.server_connected = Some(false));

    let ip = resolve(stack, url.hostname).await?;
    let mut tcp = TcpSocket::new(stack, &mut *bufs.tcp_rx, &mut *bufs.tcp_tx);
    tcp.set_timeout(Some(Duration::from_secs(15)));
    if let Err(e) = tcp.connect((ip, url.port)).await {
        // a reconnected wifi clears the "wifi lost" message even if the server is down
        if wifi_conn_sig.signaled() && wifi_conn_sig.wait().await {
            global_state
                .status
                .update(|s| s.wifi_connected = Some(true));
        }
        log::error!("connect error: {e:?}");
        return Err(WsError::Connect);
    }

    let mut socket = if url.secure {
        let mut tls = TlsConnection::new(tcp, &mut *bufs.tls_rx, &mut *bufs.tls_tx);
        tls.open(TlsContext::new(&TlsConfig::new(), InsecureNoVerify))
            .await
            .map_err(|e| {
                log::error!("tls open error: {e:?}");
                WsError::Tls
            })?;
        Socket::Tls(tls)
    } else {
        Socket::Raw(tcp)
    };

    global_state.status.update(|s| {
        s.server_connected = Some(true);
        s.wifi_connected = Some(true);
    });
    log::info!("connected!");

    let mut tx = WsTxFramer::new(true, &mut *bufs.ws_tx);
    let mut rx = WsRxFramer::new(&mut *bufs.ws_rx);
    handshake(global_state, url, &mut socket, &mut tx, &mut rx).await?;

    #[cfg(feature = "auto_add")]
    if !global_state
        .status
        .read(|s| s.device_added)
        .unwrap_or(false)
    {
        _ = send(TimerPacket {
            tag: None,
            data: TimerPacketInner::Add {
                firmware: crate::version::FIRMWARE.into(),
                sign_key: crate::state::SETTINGS.read(|s| s.sign_key),
            },
        });
    }

    let mut ota = Ota::new(FlashStorage::new(unsafe {
        esp_hal::peripherals::FLASH::steal()
    }))
    .map_err(WsError::Ota)?;
    let mut owed = Owed {
        ping: true,
        ..Owed::default()
    };

    loop {
        // socket reads time out after 15 s, so this beats well within the watchdog window
        crate::watchdog::heartbeat(crate::watchdog::TaskId::Ws);
        flush(&mut socket, &mut tx, &mut owed).await?;

        match select3(
            socket.read(rx.read_buf()),
            WAKE.wait(),
            wifi_conn_sig.wait(),
        )
        .await
        {
            Either3::First(Ok(0) | Err(_)) => return Err(WsError::Socket),
            Either3::First(Ok(n)) => rx.advance(n),
            Either3::Second(()) | Either3::Third(true) => continue,
            Either3::Third(false) => {
                global_state
                    .status
                    .update(|s| s.wifi_connected = Some(false));
                return Err(WsError::WifiDisconnected);
            }
        }

        while let Some(frame) = rx.next_frame() {
            match frame {
                Ok(WsFrame::Text(text)) => match serde_json::from_str::<TimerPacket>(text) {
                    Ok(packet) => handle_packet(global_state, packet, &mut ota, &mut owed).await?,
                    Err(e) => {
                        log::error!("timer_packet_fail: {e:?}\nTried to parse:\n{text}");
                        crate::log_once!(codes::WS_PACKET_PARSE_FAILED);
                    }
                },
                Ok(WsFrame::Binary(data)) => {
                    handle_ota_chunk(global_state, data, &mut ota, &mut owed)?
                }
                Ok(WsFrame::Ping(_)) => owed.pong = true,
                Ok(WsFrame::Close(code, reason)) => {
                    log::warn!("server closed the connection: {code} {reason}");
                    return Err(WsError::Closed);
                }
                Ok(WsFrame::Pong(_)) => {}
                Err(e) => {
                    // the framer dropped that frame; the connection goes on
                    log::error!("ws frame error: {e:?}");
                    if matches!(e, WsRxError::Oversized { .. }) {
                        crate::log_once!(codes::WS_PACKET_PARSE_FAILED);
                    }
                }
            }
        }
    }
}

async fn resolve(
    stack: Stack<'static>,
    hostname: &str,
) -> Result<embassy_net::Ipv4Address, WsError> {
    if let Ok(addr) = embassy_net::Ipv4Address::from_str(hostname) {
        return Ok(addr);
    }

    let res = stack
        .dns_query(hostname, embassy_net::dns::DnsQueryType::A)
        .await
        .map_err(|e| {
            log::error!("[WS]Dns resolver error: {e:?}");
            WsError::Dns
        })?;
    match res.first() {
        Some(IpAddress::Ipv4(addr)) => Ok(*addr),
        _ => {
            log::error!("[WS]Dns resolver empty vec");
            crate::log_once!(codes::WS_DNS_RESOLVE_EMPTY);
            Err(WsError::Dns)
        }
    }
}

/// HTTP upgrade (101 + `Sec-WebSocket-Accept`) and the `randomsigned` trust check.
async fn handshake(
    global_state: GlobalState,
    url: WsUrl<'_>,
    socket: &mut Socket<'_, '_>,
    tx: &mut WsTxFramer<'_>,
    rx: &mut WsRxFramer<'_>,
) -> Result<(), WsError> {
    let random = crate::utils::get_random_u64();
    let mut path = heapless::String::<512>::new();
    write!(
        path,
        "{}?id={}&ver={}&hw={}&firmware={}&random={}",
        url.path,
        crate::utils::get_efuse_u32(),
        crate::version::VERSION,
        crate::version::HW_VER,
        crate::version::FIRMWARE,
        random
    )
    .map_err(|_| WsError::Upgrade(0))?;

    let request = tx
        .http_upgrade_request(url.host, &path, &[])
        .ok_or(WsError::Upgrade(0))?;
    socket.write_all(request).await?;
    let accept = tx.accept_key();

    let random_signed = loop {
        let n = socket
            .read(rx.read_buf())
            .await
            .map_err(|_| WsError::Socket)?;
        if n == 0 {
            log::error!("error while reading http response");
            crate::log_once!(codes::WS_HTTP_UPGRADE_READ_FAILED);
            return Err(WsError::Socket);
        }
        rx.advance(n);

        match rx.http_response() {
            None => continue,
            Some(Err(e)) => {
                log::error!("bad http response: {e:?}");
                crate::log_once!(codes::WS_HTTP_UPGRADE_READ_FAILED);
                return Err(WsError::Upgrade(0));
            }
            Some(Ok(resp)) => {
                log::info!("http_resp_code: {}", resp.status_code);
                if !resp.is_websocket_accepted(&accept) {
                    return Err(WsError::Upgrade(resp.status_code));
                }
                break resp
                    .header("randomsigned")
                    .and_then(|v| core::str::from_utf8(v).ok()?.parse::<u128>().ok());
            }
        }
    };

    if let Some(random_signed) = random_signed {
        let mut key = [0; 16];
        key[..4].copy_from_slice(&crate::state::SETTINGS.read(|s| s.sign_key).to_be_bytes());
        let mut block = random_signed.to_be_bytes();
        global_state
            .aes
            .lock()
            .await
            .decrypt(&mut block, esp_hal::aes::Key::Key128(key));

        let (recv_random, token) = block.split_at(8);
        let recv_random = u64::from_be_bytes(recv_random.try_into().unwrap_or_default());
        let fkm_token = i32::from_be_bytes(token[..4].try_into().unwrap_or_default());
        log::debug!(
            "[trust] random: {random}, recv_random: {recv_random} | fkm_token: {fkm_token}"
        );

        if random == recv_random {
            crate::state::SESSION.write(|s| {
                s.trust_server = true;
                s.fkm_token = fkm_token;
            });
        } else {
            #[cfg(not(feature = "e2e"))]
            {
                global_state
                    .ui
                    .update(|u| u.error_text = Some("Server Not Trusted!".into()));
            }
        }
    }

    Ok(())
}

/// Replies the reader owes the server; the writer sends them before queued packets.
#[derive(Default)]
struct Owed {
    ping: bool,
    pong: bool,
    ota_ack: bool,
    crash_log: bool,
}

/// Writes everything owed or queued.
async fn flush(
    socket: &mut Socket<'_, '_>,
    tx: &mut WsTxFramer<'_>,
    owed: &mut Owed,
) -> Result<(), WsError> {
    if core::mem::take(&mut owed.ping) {
        write_frame(socket, tx.ping(&[])).await?;
    }
    if core::mem::take(&mut owed.pong) {
        write_frame(socket, tx.pong(&[])).await?;
    }
    if core::mem::take(&mut owed.ota_ack) {
        write_frame(socket, tx.binary(&[])).await?;
    }
    if core::mem::take(&mut owed.crash_log) {
        // shortcut: the log lock can't be held across the write, so the dump (≤ 2 KB, only on
        // the server's debug command) is copied to the heap; a framer API to re-borrow the
        // built frame would avoid it
        let dump = crate::utils::error_log::with_error_log(|log| {
            let mut dump = alloc::vec::Vec::with_capacity(log.len() + 1);
            dump.push(b'C');
            dump.extend_from_slice(log);
            dump
        });
        write_chunked(socket, tx, &dump).await?;
    }

    while let Ok(packet) = OUT.try_receive() {
        match tx.text_with(|buf| serde_json_core::to_slice(&packet, buf).ok()) {
            Some(frame) => socket.write_all(frame).await?,
            None => {
                log::error!("packet doesn't fit the tx buffer");
                crate::log_once!(codes::WS_PACKET_SERIALIZE_FAILED);
            }
        }
    }

    if let Some(logs) = PENDING_LOGS.lock(Cell::take) {
        write_chunked(socket, tx, &logs).await?;
    }

    if let Some((code, reason)) = PENDING_CLOSE.lock(Cell::take) {
        write_frame(socket, tx.close(code, reason)).await?;
        if code == CLOSE_SLEEP {
            // the radio is switched off within 15 s; don't reconnect before that
            Timer::after_millis(30_000).await;
        }
    }

    Ok(())
}

/// Binary message, split into frames as large as the tx buffer allows.
async fn write_chunked(
    socket: &mut Socket<'_, '_>,
    tx: &mut WsTxFramer<'_>,
    data: &[u8],
) -> Result<(), WsError> {
    let frame = WsFrame::Binary(data);
    let mut offset = 0;
    while let Some((part, more)) = tx.frame_chunk(&frame, &mut offset) {
        socket.write_all(part).await?;
        if !more {
            break;
        }
    }
    Ok(())
}

/// Writes a control frame built by the framer (empty ones always fit).
async fn write_frame(socket: &mut Socket<'_, '_>, frame: Option<&[u8]>) -> Result<(), WsError> {
    match frame {
        Some(frame) => socket.write_all(frame).await,
        None => Ok(()),
    }
}

/// Side effects of a received packet; tagged ones also go to the waiting request.
async fn handle_packet(
    global_state: GlobalState,
    packet: TimerPacket,
    ota: &mut Ota<FlashStorage<'static>>,
    owed: &mut Owed,
) -> Result<(), WsError> {
    match &packet.data {
        TimerPacketInner::DeviceSettings {
            added,
            locales,
            default_locale,
            fkm_token,
            secure_rfid,
            auto_setup,
            sound_enabled,
        } => {
            global_state
                .status
                .update(|s| s.device_added = Some(*added));
            crate::translations::clear_locales();
            for locale in locales {
                crate::translations::process_locale(&locale.locale, &locale.translations);
            }
            crate::translations::select_locale(default_locale);
            crate::translations::set_default_locale();

            crate::state::SETTINGS.write(|s| s.sound_enabled = *sound_enabled);
            crate::state::SESSION.write(|s| {
                s.fkm_token = *fkm_token;
                s.secure_rfid = *secure_rfid;
                s.auto_setup = *auto_setup;
            });
        }
        TimerPacketInner::ApiError(e) => {
            global_state
                .ui
                .update(|u| u.error_text = Some(e.error.clone()));
        }
        TimerPacketInner::CustomMessage { line1, line2 } => {
            global_state
                .ui
                .update(|u| u.custom_message = Some((line1.clone(), line2.clone())));
        }
        TimerPacketInner::EpochTime { current_epoch } => crate::state::set_epoch(*current_epoch),
        TimerPacketInner::StartUpdate {
            version,
            size,
            crc,
            firmware,
            ..
        } => {
            if firmware.as_str() != crate::version::FIRMWARE {
                return Ok(());
            }
            #[cfg(not(feature = "e2e"))]
            if !crate::state::SESSION.read(|s| s.trust_server) {
                return Ok(());
            }

            log::info!("Start update: {firmware}/{version} size: {size} crc: {crc}");
            ota.begin(*size, *crc).map_err(WsError::Ota)?;
            OTA_ACTIVE.store(true, Ordering::Relaxed);
            global_state.solve.update(|s| s.scene = Scene::Update);

            OUT.clear();
            owed.ota_ack = true;
        }
        #[cfg(feature = "v4")]
        TimerPacketInner::SetDeviceSettings {
            volume: Some(volume),
        } => {
            let volume = (*volume).clamp(
                crate::consts::BUZZER_VOLUME_MIN,
                crate::consts::BUZZER_VOLUME_MAX,
            );
            crate::state::SETTINGS.write(|s| s.buzzer_volume = volume);
            if let Err(e) = global_state
                .nvs
                .set(&crate::consts::NVS_BUZZER_VOLUME, volume)
                .await
            {
                log::error!("Cannot save buzzer volume to NVS: {e:?}");
                crate::log_once!(codes::NVS_BUZZER_VOLUME_WRITE_FAILED);
            }
        }
        TimerPacketInner::DumpCrashLog => owed.crash_log = true,
        #[cfg(feature = "e2e")]
        TimerPacketInner::TestPacket(test_packet) => {
            parse_test_packet(test_packet.clone(), &global_state).await;
        }
        _ => {}
    }

    let tag = match (&packet.data, packet.tag) {
        (TimerPacketInner::DelegateResponse(_), None) => Some(DELEGATE_TAG.load(Ordering::Acquire)),
        (_, tag) => tag,
    };
    if let Some(tag) = tag.filter(|&t| t != 0)
        && let Some(slot) = SLOTS.iter().find(|s| s.tag.load(Ordering::Acquire) == tag)
    {
        slot.response.signal(packet);
    }
    Ok(())
}

/// A binary frame is the next OTA chunk (acked so the server sends the next one).
fn handle_ota_chunk(
    global_state: GlobalState,
    data: &[u8],
    ota: &mut Ota<FlashStorage<'static>>,
    owed: &mut Owed,
) -> Result<(), WsError> {
    if !ota_active() {
        return Ok(());
    }
    #[cfg(not(feature = "e2e"))]
    if !crate::state::SESSION.read(|s| s.trust_server) {
        return Ok(());
    }

    if ota.write(data).map_err(WsError::Ota)? {
        log::info!("OTA complete! Verifying..");
        if let Err(e) = ota.finish(true) {
            log::error!("OTA finish failed: {e:?}");
            crate::utils::error_log::add_error(codes::OTA_VERIFY_FAILED);
            return Err(WsError::Ota(e));
        }
        log::info!("OTA restart!");
        esp_hal::system::software_reset();
    }

    global_state
        .update_progress
        .signal(ota.progress().map_or(0, |p| p.percent()));
    owed.ota_ack = true;
    Ok(())
}

#[cfg(feature = "e2e")]
async fn parse_test_packet(
    test_packet: crate::structs::TestPacketData,
    global_state: &GlobalState,
) {
    use crate::structs::TestPacketData;
    use crate::utils::stackmat::StackmatTimerState;

    log::warn!("TEST PACKET: {test_packet:?}");
    let e2e = &global_state.e2e;
    match test_packet {
        TestPacketData::ResetState => {
            global_state.reset_solve(false).await;
            e2e.stackmat_sig.signal((StackmatTimerState::Reset, 0));
            send_test_ack(global_state);
        }
        TestPacketData::HardStateReset => {
            e2e.stackmat_sig.signal((StackmatTimerState::Reset, 0));
            global_state.hard_reset();
        }
        TestPacketData::ScanCard(uid) => e2e.card_scan_sig.signal(uid as u128),
        TestPacketData::ButtonPress { pin, press_time } => {
            e2e.buttons_sig.signal((pin as usize, press_time))
        }
        TestPacketData::StackmatTime(ms) => {
            e2e.stackmat_sig.signal((StackmatTimerState::Running, ms))
        }
        TestPacketData::StackmatReset => e2e.stackmat_sig.signal((StackmatTimerState::Reset, 0)),
    }
}

// boxing the TLS connection would need the heap; only one Socket exists at a time
#[allow(clippy::large_enum_variant)]
enum Socket<'a, 'b> {
    Tls(TlsConnection<'b, TcpSocket<'a>, Aes128GcmSha256>),
    Raw(TcpSocket<'a>),
}

impl Socket<'_, '_> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        match self {
            Socket::Tls(tls) => tls.read(buf).await.map_err(|_| ()),
            Socket::Raw(tcp) => tcp.read(buf).await.map_err(|_| ()),
        }
    }

    async fn write_all(&mut self, mut buf: &[u8]) -> Result<(), WsError> {
        while !buf.is_empty() {
            let n = match self {
                Socket::Tls(tls) => tls.write(buf).await.map_err(|_| WsError::Socket)?,
                Socket::Raw(tcp) => tcp.write(buf).await.map_err(|_| WsError::Socket)?,
            };
            if n == 0 {
                return Err(WsError::Socket);
            }
            buf = &buf[n..];
        }
        if let Socket::Tls(tls) = self {
            tls.flush().await.map_err(|_| WsError::Socket)?;
        }
        Ok(())
    }
}

/// RNG for embedded-tls (embassy-crypto)
struct EspRng;

impl embassy_crypto::driver::Rng for EspRng {
    fn fill_bytes(buf: &mut [u8]) {
        _ = crate::custom_rng(buf);
    }
}

embassy_crypto::rng_impl!(EspRng);
