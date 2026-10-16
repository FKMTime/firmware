//! BLE link to an FKM display: scan → pair/bond → stream the timer value over GATT.
//!
//! One task, phases as plain async fns: `run` (idle until there is a bond or a scan request)
//! → `session` (one radio/host stack) → `scan` (only without a bond) → `connect_loop`.
//! Every await on the peer has a timeout, reconnects back off, and UI commands arrive through
//! a queue so they can't overwrite each other.

mod codec;

use crate::{
    consts::NVS_BONDING_KEY,
    state::{GlobalState, MenuScene},
    structs::{BleDisplayDevice, MAX_BLE_DEVICES},
    utils::error_log::codes,
};
use alloc::{rc::Rc, vec::Vec};
use core::cell::{Cell, RefCell};
use embassy_futures::select::{Either, select, select3};
use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex},
    channel::Channel,
    signal::Signal,
};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::peripherals::BT;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::*;

const DISPLAY_SERVICE: u128 = 0xa5bad9f2700a4c3db9e2e58ad262d40e;
const TIME_CHARACTERISTIC: u128 = 0xa5178cade4e045988053a4a78b9281e2;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PAIRING_TIMEOUT: Duration = Duration::from_secs(10);
const GATT_TIMEOUT: Duration = Duration::from_secs(5);
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(8);
/// HCI disconnect reason "authentication failure": the display no longer knows our bond.
const AUTH_FAILURE: u8 = 0x05;

type Ctrl = ExternalController<BleConnector<'static>, 20>;
type Pool = DefaultPacketPool;

/// UI → BLE task commands.
pub static COMMANDS: BleCommands = BleCommands::new();

#[derive(Debug, Clone, PartialEq)]
pub enum BleAction {
    Connect(BleDisplayDevice),
    StartScan,
    Unpair,
}

/// UI → BLE task commands. A queue, so StartScan/Connect/Unpair can't overwrite each other.
pub struct BleCommands(Channel<CriticalSectionRawMutex, BleAction, 4>);

impl BleCommands {
    pub const fn new() -> Self {
        Self(Channel::new())
    }

    /// Queues a command (named like the `Signal` it replaced, so callers stay unchanged).
    #[cfg_attr(feature = "qa", allow(dead_code))] // qa builds have no BLE menu
    pub fn signal(&self, cmd: BleAction) {
        if self.0.try_send(cmd).is_err() {
            log::warn!("[ble] command queue full, dropped");
        }
    }

    async fn wait_for(&self, mut accept: impl FnMut(&BleAction) -> bool) -> BleAction {
        loop {
            let cmd = self.0.receive().await;
            if accept(&cmd) {
                return cmd;
            }
        }
    }
}

#[embassy_executor::task]
pub async fn bluetooth_timer_task(
    bt: BT<'static>,
    state: GlobalState,
    sleep_sig: Rc<Signal<CriticalSectionRawMutex, bool>>,
) {
    loop {
        // sleeping drops the whole BLE stack
        select(run(&bt, state), wait_sleep(&sleep_sig, true)).await;
        // let the controller shut down before it can be created again
        Timer::after_millis(1000).await;
        wait_sleep(&sleep_sig, false).await;
    }
}

async fn wait_sleep(sig: &Signal<CriticalSectionRawMutex, bool>, sleep: bool) {
    while sig.wait().await != sleep {}
}

async fn run(bt: &BT<'static>, state: GlobalState) {
    let mut backoff = Backoff::new();
    loop {
        let bond = load_bond(state).await;
        if bond.is_none() {
            wait_for_scan_request(state).await;
        }

        match session(bt, state, bond).await {
            Ok(()) => backoff.reset(),
            Err(()) => backoff.wait().await,
        }
    }
}

async fn wait_for_scan_request(state: GlobalState) {
    // the pairing menu is already open (e.g. after unpairing from it)
    if state.ui.read(|u| u.menu_scene) == Some(MenuScene::BtDisplay) {
        return;
    }
    COMMANDS.wait_for(|c| matches!(c, BleAction::StartScan))
        .await;
}

/// One controller + host stack; `Err` means it couldn't start or the host stopped.
async fn session(
    bt: &BT<'static>,
    state: GlobalState,
    bond: Option<BondInformation>,
) -> Result<(), ()> {
    // SAFETY: only one session (and so one connector) exists at a time; the previous one is
    // dropped before `run` creates the next.
    let connector = BleConnector::new(unsafe { bt.clone_unchecked() }, Default::default())
        .map_err(|e| {
            log::error!("[ble] cannot init connector ({e:?})");
            crate::log_once!(codes::BLE_INIT_FAILED);
        })?;
    let mac: [u8; 6] = esp_hal::efuse::base_mac_address()
        .as_bytes()
        .try_into()
        .map_err(|_| {
            log::error!("[ble] cannot read mac address");
            crate::log_once!(codes::BLE_MAC_READ_FAILED);
        })?;

    let mut resources: HostResources<Ctrl, Pool, 1, 3> = HostResources::new();
    let stack = trouble_host::new(ExternalController::new(connector), &mut resources)
        .set_random_address(Address::random(mac))
        .build();
    let mut runner = stack.runner();
    let discovery = Discovery::new();

    match select(
        runner.run_with_handler(&discovery),
        link(&stack, state, bond, &discovery),
    )
    .await
    {
        Either::First(res) => {
            log::error!("[ble] host stopped ({res:?})");
            Err(())
        }
        Either::Second(res) => res,
    }
}

async fn link(
    stack: &Stack<'_, Ctrl, Pool>,
    state: GlobalState,
    bond: Option<BondInformation>,
    discovery: &Discovery,
) -> Result<(), ()> {
    let mut central = stack.central();
    let (target, bond) = match bond {
        Some(bond) => {
            stack.add_bond_information(bond.clone()).map_err(|e| {
                log::error!("[ble] add bond failed ({e:?})");
                crate::log_once!(codes::BLE_BOND_ADD_FAILED);
            })?;
            (bond.identity.addr, Some(bond))
        }
        None => {
            let (c, picked) = scan(central, state, discovery).await;
            central = c;
            match picked? {
                Some(device) => (Address::random(device.addr), None),
                None => return Ok(()),
            }
        }
    };

    // re-pairing or unpairing from the menu ends the link (the buttons already dropped the bond)
    let ended = COMMANDS.wait_for(|c| matches!(c, BleAction::Unpair | BleAction::StartScan));
    select(
        ended,
        connect_loop(stack, &mut central, target, bond, state),
    )
    .await;
    Ok(())
}

/// Lists displays in the menu until the user picks one (`Ok(None)` when they unpair instead).
async fn scan<'a>(
    central: Central<'a, Ctrl, Pool>,
    state: GlobalState,
    discovery: &Discovery,
) -> (
    Central<'a, Ctrl, Pool>,
    Result<Option<BleDisplayDevice>, ()>,
) {
    state.ui.update(|u| {
        u.discovered_bluetooth_devices.clear();
        u.selected_bluetooth_item = 0;
    });

    let mut scanner = Scanner::new(central);
    let picked = match scanner.scan(&ScanConfig::default()).await {
        Ok(_session) => {
            discovery.start();
            let picked = COMMANDS.wait_for(|c| matches!(c, BleAction::Connect(_) | BleAction::Unpair));
            let picked = match select(picked, list_displays(state, discovery)).await {
                Either::First(BleAction::Connect(device)) => Some(device),
                _ => None,
            };
            discovery.stop();
            Ok(picked)
        }
        Err(e) => {
            log::error!("[ble] cannot start scan ({e:?})");
            crate::log_once!(codes::BLE_SCAN_START_FAILED);
            Err(())
        }
    };
    (scanner.into_inner(), picked)
}

async fn list_displays(state: GlobalState, discovery: &Discovery) {
    loop {
        let device = discovery.found.receive().await;
        state.ui.update(|u| {
            // keep the cursor on "Unpair"/"Exit" (listed after the devices) when it's there
            let cursor_after_list = u.selected_bluetooth_item > 0
                && u.selected_bluetooth_item >= u.discovered_bluetooth_devices.len();
            if u.discovered_bluetooth_devices.push(device) && cursor_after_list {
                u.selected_bluetooth_item += 1;
            }
        });
    }
}

async fn connect_loop(
    stack: &Stack<'_, Ctrl, Pool>,
    central: &mut Central<'_, Ctrl, Pool>,
    target: Address,
    mut bond: Option<BondInformation>,
    state: GlobalState,
) {
    let config = ConnectConfig {
        connect_params: Default::default(),
        scan_config: ScanConfig {
            filter_accept_list: &[target],
            ..Default::default()
        },
    };

    let mut backoff = Backoff::new();
    loop {
        match connect_once(stack, central, &config, &mut bond, state).await {
            Attempt::Retry { worked } => {
                if worked {
                    backoff.reset();
                }
                backoff.wait().await;
            }
            Attempt::Unbond => {
                log::warn!("[ble] bond rejected, forgetting it");
                _ = state.nvs.delete(&NVS_BONDING_KEY).await;
                return;
            }
        }
    }
}

enum Attempt {
    /// `worked`: the link reached the streaming state before it ended
    Retry { worked: bool },
    /// the display refused our keys; it has to be paired again
    Unbond,
}

async fn connect_once(
    stack: &Stack<'_, Ctrl, Pool>,
    central: &mut Central<'_, Ctrl, Pool>,
    config: &ConnectConfig<'_>,
    bond: &mut Option<BondInformation>,
    state: GlobalState,
) -> Attempt {
    const FAILED: Attempt = Attempt::Retry { worked: false };

    let conn = match with_timeout(CONNECT_TIMEOUT, central.connect(config)).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(e)) => {
            log::warn!("[ble] connect failed ({e:?})");
            return FAILED;
        }
        Err(_) => return FAILED, // display off or out of range
    };

    // only allow bonding when there is no bond yet
    if let Err(e) = conn.set_bondable(bond.is_none()) {
        log::error!("[ble] set bondable failed ({e:?})");
        crate::log_once!(codes::BLE_BONDABLE_FAILED);
        return FAILED;
    }
    if let Err(e) = conn.request_security() {
        log::error!("[ble] request security failed ({e:?})");
        crate::log_once!(codes::BLE_REQUEST_SECURITY_FAILED);
        return FAILED;
    }

    match with_timeout(PAIRING_TIMEOUT, pairing(&conn)).await {
        Ok(Pairing::Done {
            encrypted,
            new_bond,
        }) => {
            if let Some(new_bond) = new_bond {
                store_bond(state, &new_bond).await;
                *bond = Some(new_bond);
            }
            if !encrypted {
                return Attempt::Unbond;
            }
        }
        // the display may still take unencrypted writes, so carry on as before
        Ok(Pairing::Failed) => {}
        Ok(Pairing::Rejected) => return Attempt::Unbond,
        Ok(Pairing::Disconnected) => return FAILED,
        Err(_) => {
            log::warn!("[ble] pairing timed out");
            return FAILED;
        }
    }

    Attempt::Retry {
        worked: stream(stack, &conn, state).await,
    }
}

enum Pairing {
    Done {
        encrypted: bool,
        new_bond: Option<BondInformation>,
    },
    Failed,
    Rejected,
    Disconnected,
}

async fn pairing(conn: &Connection<'_, Pool>) -> Pairing {
    loop {
        match conn.next().await {
            ConnectionEvent::PairingComplete {
                security_level,
                bond,
            } => {
                log::info!("[ble] pairing complete: {security_level:?}");
                return Pairing::Done {
                    encrypted: security_level.encrypted(),
                    new_bond: bond,
                };
            }
            ConnectionEvent::PairingFailed(e) => {
                log::error!("[ble] pairing failed ({e:?})");
                crate::log_once!(codes::BLE_PAIRING_FAILED);
                return Pairing::Failed;
            }
            ConnectionEvent::Disconnected { reason } => {
                log::warn!("[ble] disconnected while pairing ({reason:?})");
                return match reason.into_inner() {
                    AUTH_FAILURE => Pairing::Rejected,
                    _ => Pairing::Disconnected,
                };
            }
            _ => {}
        }
    }
}

/// Streams timer values until the link breaks; returns whether streaming started.
async fn stream(
    stack: &Stack<'_, Ctrl, Pool>,
    conn: &Connection<'_, Pool>,
    state: GlobalState,
) -> bool {
    let client = match with_timeout(GATT_TIMEOUT, GattClient::<_, Pool, 10>::new(stack, conn)).await
    {
        Ok(Ok(client)) => client,
        _ => {
            log::error!("[ble] gatt client failed");
            crate::log_once!(codes::BLE_GATT_CLIENT_FAILED);
            return false;
        }
    };

    let streaming = Cell::new(false);
    select3(
        client.task(),
        disconnected(conn),
        write_times(&client, state, &streaming),
    )
    .await;
    streaming.get()
}

async fn disconnected(conn: &Connection<'_, Pool>) {
    loop {
        if let ConnectionEvent::Disconnected { reason } = conn.next().await {
            log::info!("[ble] disconnected ({reason:?})");
            return;
        }
    }
}

async fn write_times(
    client: &GattClient<'_, Ctrl, Pool, 10>,
    state: GlobalState,
    streaming: &Cell<bool>,
) {
    let service = Uuid::from(DISPLAY_SERVICE);
    let Ok(Ok(services)) = with_timeout(GATT_TIMEOUT, client.services_by_uuid(&service)).await
    else {
        log::warn!("[ble] service discovery failed");
        return;
    };
    let Some(service) = services.first() else {
        log::error!("[ble] display service not found");
        crate::log_once!(codes::BLE_SERVICE_NOT_FOUND);
        return;
    };

    let characteristic = Uuid::from(TIME_CHARACTERISTIC);
    let Ok(Ok(characteristic)) = with_timeout(
        GATT_TIMEOUT,
        client.characteristic_by_uuid::<u64>(service, &characteristic),
    )
    .await
    else {
        log::error!("[ble] time characteristic not found");
        crate::log_once!(codes::BLE_CHARACTERISTIC_NOT_FOUND);
        return;
    };

    let Some(mut timer) = state.timer.receiver() else {
        log::error!("[ble] no timer receiver left");
        return;
    };
    streaming.set(true);
    loop {
        let time = timer.changed().await;
        if client
            .write_characteristic_without_response(&characteristic, &time.to_be_bytes())
            .await
            .is_err()
        {
            return;
        }
    }
}

/// Collects FKM displays from advertising reports while `scan` runs.
struct Discovery {
    scanning: Cell<bool>,
    seen: RefCell<heapless::Vec<[u8; 6], MAX_BLE_DEVICES>>,
    found: Channel<NoopRawMutex, BleDisplayDevice, MAX_BLE_DEVICES>,
}

impl Discovery {
    fn new() -> Self {
        Self {
            scanning: Cell::new(false),
            seen: RefCell::new(heapless::Vec::new()),
            found: Channel::new(),
        }
    }

    fn start(&self) {
        if let Ok(mut seen) = self.seen.try_borrow_mut() {
            seen.clear();
        }
        self.found.clear();
        self.scanning.set(true);
    }

    fn stop(&self) {
        self.scanning.set(false);
    }
}

impl EventHandler for Discovery {
    fn on_adv_reports(&self, mut reports: LeAdvReportsIter<'_>) {
        if !self.scanning.get() {
            return;
        }
        let Ok(mut seen) = self.seen.try_borrow_mut() else {
            return;
        };

        while let Some(Ok(report)) = reports.next() {
            let addr = report.addr.into_inner();
            if seen.contains(&addr) {
                continue;
            }
            let Some(name) =
                codec::parse_device_name(report.data).filter(|n| codec::is_fkm_display(n))
            else {
                continue;
            };

            if seen.push(addr).is_ok() {
                log::info!("[ble] found display {name}");
                _ = self.found.try_send(BleDisplayDevice {
                    name: name.into(),
                    addr,
                });
            }
        }
    }
}

struct Backoff(Duration);

impl Backoff {
    fn new() -> Self {
        Self(MIN_BACKOFF)
    }

    fn reset(&mut self) {
        self.0 = MIN_BACKOFF;
    }

    async fn wait(&mut self) {
        Timer::after(self.0).await;
        self.0 = (self.0 * 2).min(MAX_BACKOFF);
    }
}

async fn store_bond(state: GlobalState, bond: &BondInformation) {
    let record = codec::BondRecord {
        addr: bond.identity.addr.addr.into_inner(),
        random_addr: bond.identity.addr.kind == AddrKind::RANDOM,
        ltk: bond.ltk.to_le_bytes(),
        security_level: match bond.security_level {
            SecurityLevel::NoEncryption => 0,
            SecurityLevel::Encrypted => 1,
            SecurityLevel::EncryptedAuthenticated => 2,
        },
    };

    let buf = codec::encode_bond(&record);
    if let Err(e) = state.nvs.set(&NVS_BONDING_KEY, buf.as_slice()).await {
        log::error!("[ble] storing bond failed ({e:?})");
        crate::log_once!(codes::NVS_BONDING_KEY_WRITE_FAILED);
    }
}

async fn load_bond(state: GlobalState) -> Option<BondInformation> {
    let buf = state.nvs.get::<Vec<u8>>(&NVS_BONDING_KEY).await.ok()?;
    let record = codec::decode_bond(&buf)?;

    let addr = match record.random_addr {
        true => Address::random(record.addr),
        false => Address {
            kind: AddrKind::PUBLIC,
            addr: BdAddr::new(record.addr),
        },
    };
    let security_level = match record.security_level {
        0 => SecurityLevel::NoEncryption,
        1 => SecurityLevel::Encrypted,
        _ => SecurityLevel::EncryptedAuthenticated,
    };
    Some(BondInformation {
        identity: Identity { addr, irk: None },
        security_level,
        is_bonded: true,
        ltk: LongTermKey::from_le_bytes(record.ltk),
    })
}
