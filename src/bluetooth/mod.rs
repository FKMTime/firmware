//! BLE link to an FKM display: pick a display from a scan, pair once (the display only accepts
//! its bonded timer), then stream every timer value as a GATT write.

mod codec;

use crate::{
    consts::NVS_BONDING_KEY,
    state::{GlobalState, MenuScene},
    structs::BleDisplayDevice,
    utils::error_log::codes,
};
use alloc::vec::Vec;
use core::cell::Cell;
use embassy_futures::select::{Either, select, select3};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::peripherals::BT;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::*;

const DISPLAY_SERVICE: u128 = 0xa5bad9f2700a4c3db9e2e58ad262d40e;
const TIME_CHARACTERISTIC: u128 = 0xa5178cade4e045988053a4a78b9281e2;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_secs(8);
/// HCI disconnect reason "authentication failure": the display no longer knows our bond.
const AUTH_FAILURE: u8 = 0x05;

type Ctrl = ExternalController<BleConnector<'static>, 20>;
type Pool = DefaultPacketPool;
type Resources = HostResources<Ctrl, Pool, 1, 3>;

// in a static, not in the task future: futures are built on the stack and then moved, so their
// size is a stack spike on every poll (trouble_host::new re-initialises it for each session)
static RESOURCES: static_cell::StaticCell<Resources> = static_cell::StaticCell::new();

/// UI → BLE task commands.
pub static COMMANDS: BleCommands = BleCommands(Channel::new());
/// `true` = going to sleep (BLE stack dropped until `false`), sent by [`crate::power::Supervisor`].
pub static SLEEP: crate::power::SleepSignal = Signal::new();

#[derive(Debug, Clone, PartialEq)]
pub enum BleAction {
    Connect(BleDisplayDevice),
    StartScan,
    Unpair,
}

pub struct BleCommands(Channel<CriticalSectionRawMutex, BleAction, 4>);

impl BleCommands {
    #[cfg_attr(feature = "qa", allow(dead_code))] // qa builds have no BLE menu
    pub fn signal(&self, cmd: BleAction) {
        if self.0.try_send(cmd).is_err() {
            log::warn!("[ble] command queue full, dropped");
        }
    }

    async fn wait_for(&self, accept: impl Fn(&BleAction) -> bool) -> BleAction {
        loop {
            let cmd = self.0.receive().await;
            if accept(&cmd) {
                return cmd;
            }
        }
    }
}

#[embassy_executor::task]
pub async fn bluetooth_timer_task(bt: BT<'static>, state: GlobalState) {
    // all fields are MaybeUninit, so this write is free (no stack temporary)
    let Some(resources) = RESOURCES.try_uninit() else {
        return;
    };
    let resources = resources.write(HostResources::new());
    loop {
        // sleeping drops the whole BLE stack
        select(run(&bt, state, resources), wait_sleep(true)).await;
        // let the controller shut down before it is created again
        Timer::after_millis(1000).await;
        wait_sleep(false).await;
    }
}

async fn wait_sleep(sleep: bool) {
    while SLEEP.wait().await != sleep {}
}

async fn run(bt: &BT<'static>, state: GlobalState, resources: &mut Resources) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let bond = load_bond(state).await;
        if bond.is_none() && state.ui.read(|u| u.menu_scene) != Some(MenuScene::BtDisplay) {
            COMMANDS.wait_for(|c| *c == BleAction::StartScan).await;
        }

        match session(bt, state, bond, resources).await {
            Ok(()) => backoff = Duration::from_secs(1),
            Err(()) => {
                Timer::after(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// One controller + host stack, until the user picks/unpairs or the stack fails (`Err`).
async fn session(
    bt: &BT<'static>,
    state: GlobalState,
    bond: Option<BondInformation>,
    resources: &mut Resources,
) -> Result<(), ()> {
    // SAFETY: only one session (and so one connector) exists at a time.
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

    let stack = trouble_host::new(ExternalController::new(connector), resources)
        .set_random_address(Address::random(codec::static_address_from_mac(mac)))
        .build();
    let mut runner = stack.runner();
    let discovery = Discovery {
        state,
        scanning: Cell::new(false),
    };

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
    mut bond: Option<BondInformation>,
    discovery: &Discovery,
) -> Result<(), ()> {
    let mut central = stack.central();
    let target = match &bond {
        Some(b) => {
            stack.add_bond_information(b.clone()).map_err(|e| {
                log::error!("[ble] add bond failed ({e:?})");
                crate::log_once!(codes::BLE_BOND_ADD_FAILED);
            })?;
            b.identity.addr
        }
        None => {
            state.ui.update(|u| {
                u.discovered_bluetooth_devices.clear();
                u.selected_bluetooth_item = 0;
            });
            let mut scanner = Scanner::new(central);
            let picked = match scanner.scan(&ScanConfig::default()).await {
                Ok(_session) => {
                    discovery.scanning.set(true);
                    let cmd = COMMANDS
                        .wait_for(|c| matches!(c, BleAction::Connect(_) | BleAction::Unpair))
                        .await;
                    discovery.scanning.set(false);
                    cmd
                }
                Err(e) => {
                    log::error!("[ble] cannot start scan ({e:?})");
                    crate::log_once!(codes::BLE_SCAN_START_FAILED);
                    return Err(());
                }
            };
            central = scanner.into_inner();
            match picked {
                BleAction::Connect(device) => match device.random_address {
                    true => Address::random(device.addr),
                    false => Address::new(AddrKind::PUBLIC, BdAddr::new(device.addr)),
                },
                _ => return Ok(()),
            }
        }
    };

    let config = ConnectConfig {
        connect_params: Default::default(),
        scan_config: ScanConfig {
            filter_accept_list: &[target],
            ..Default::default()
        },
    };
    // re-pairing or unpairing from the menu ends the link; unpairing also forgets the bond
    let ended = async {
        if COMMANDS
            .wait_for(|c| matches!(c, BleAction::Unpair | BleAction::StartScan))
            .await
            == BleAction::Unpair
        {
            _ = state.nvs.delete(&NVS_BONDING_KEY).await;
        }
    };
    let connect = async {
        let mut backoff = Duration::from_secs(1);
        loop {
            match connect_once(stack, &mut central, &config, &mut bond, state).await {
                Some(true) => backoff = Duration::from_secs(1),
                Some(false) => {}
                None => {
                    log::warn!("[ble] bond rejected, forgetting it");
                    _ = state.nvs.delete(&NVS_BONDING_KEY).await;
                    return;
                }
            }
            Timer::after(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    };
    select(ended, connect).await;
    Ok(())
}

/// One connection: `Some(streamed)` to retry, `None` when the display rejected our bond.
async fn connect_once(
    stack: &Stack<'_, Ctrl, Pool>,
    central: &mut Central<'_, Ctrl, Pool>,
    config: &ConnectConfig<'_>,
    bond: &mut Option<BondInformation>,
    state: GlobalState,
) -> Option<bool> {
    let conn = match with_timeout(TIMEOUT, central.connect(config)).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(e)) => {
            log::warn!("[ble] connect failed ({e:?})");
            return Some(false);
        }
        Err(_) => return Some(false), // display off or out of range
    };

    // bond only once; afterwards the stored keys just re-encrypt the link
    if conn
        .set_bondable(bond.is_none())
        .and_then(|_| conn.request_security())
        .is_err()
    {
        log::error!("[ble] cannot request security");
        crate::log_once!(codes::BLE_REQUEST_SECURITY_FAILED);
        return Some(false);
    }

    let paired = with_timeout(TIMEOUT, async {
        loop {
            match conn.next().await {
                ConnectionEvent::PairingComplete {
                    security_level,
                    bond,
                } => {
                    break Ok((security_level.encrypted(), bond));
                }
                // Resuming a stored bond encrypts the link without pairing again.
                // Fresh pairings must still wait for PairingComplete to save their keys.
                ConnectionEvent::Encrypted { security_level, .. } if bond.is_some() => {
                    break Ok((security_level.encrypted(), None));
                }
                ConnectionEvent::PairingFailed(e) => {
                    log::error!("[ble] pairing failed ({e:?})");
                    crate::log_once!(codes::BLE_PAIRING_FAILED);
                    // the display still shows unencrypted writes
                    break Ok((true, None));
                }
                ConnectionEvent::Disconnected { reason } => break Err(reason.into_inner()),
                _ => {}
            }
        }
    })
    .await;

    match paired {
        Ok(Ok((encrypted, new_bond))) => {
            if let Some(new_bond) = new_bond {
                store_bond(state, &new_bond).await;
                *bond = Some(new_bond);
            }
            if !encrypted {
                return None;
            }
        }
        Ok(Err(AUTH_FAILURE)) => return None,
        Ok(Err(_)) | Err(_) => return Some(false),
    }

    Some(stream(stack, &conn, state).await)
}

/// Writes timer values until the link breaks; returns whether streaming started.
async fn stream(
    stack: &Stack<'_, Ctrl, Pool>,
    conn: &Connection<'_, Pool>,
    state: GlobalState,
) -> bool {
    let Ok(Ok(client)) = with_timeout(TIMEOUT, GattClient::<_, Pool, 10>::new(stack, conn)).await
    else {
        log::error!("[ble] gatt client failed");
        crate::log_once!(codes::BLE_GATT_CLIENT_FAILED);
        return false;
    };

    let streaming = Cell::new(false);
    let write = async {
        let Ok(Ok(services)) = with_timeout(
            TIMEOUT,
            client.services_by_uuid(&Uuid::from(DISPLAY_SERVICE)),
        )
        .await
        else {
            log::warn!("[ble] service discovery failed");
            return;
        };
        let Some(service) = services.first() else {
            log::error!("[ble] display service not found");
            crate::log_once!(codes::BLE_SERVICE_NOT_FOUND);
            return;
        };
        let Ok(Ok(time)) = with_timeout(
            TIMEOUT,
            client.characteristic_by_uuid::<u64>(service, &Uuid::from(TIME_CHARACTERISTIC)),
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
            let ms = timer.changed().await;
            if client
                .write_characteristic_without_response(&time, &ms.to_be_bytes())
                .await
                .is_err()
            {
                return;
            }
        }
    };
    let disconnected =
        async { while !matches!(conn.next().await, ConnectionEvent::Disconnected { .. }) {} };

    select3(client.task(), disconnected, write).await;
    streaming.get()
}

/// Lists FKM displays in the menu while a scan runs.
struct Discovery {
    state: GlobalState,
    scanning: Cell<bool>,
}

impl EventHandler for Discovery {
    fn on_adv_reports(&self, mut reports: LeAdvReportsIter<'_>) {
        if !self.scanning.get() {
            return;
        }
        while let Some(Ok(report)) = reports.next() {
            let addr = report.addr.into_inner();
            let random_address = match report.addr_kind {
                AddrKind::PUBLIC => false,
                AddrKind::RANDOM => true,
                _ => continue,
            };
            let Some(name) =
                codec::parse_device_name(report.data).filter(|n| codec::is_fkm_display(n))
            else {
                continue;
            };
            if self.state.ui.read(|u| {
                u.discovered_bluetooth_devices
                    .iter()
                    .any(|d| d.addr == addr && d.random_address == random_address)
            }) {
                continue;
            }

            log::info!("[ble] found display {name}");
            self.state.ui.update(|u| {
                // keep the cursor on "Unpair"/"Exit" (listed after the devices) when it's there
                let after_list = u.selected_bluetooth_item > 0
                    && u.selected_bluetooth_item >= u.discovered_bluetooth_devices.len();
                let device = BleDisplayDevice {
                    name: name.into(),
                    addr,
                    random_address,
                };
                if u.discovered_bluetooth_devices.push(device) && after_list {
                    u.selected_bluetooth_item += 1;
                }
            });
        }
    }
}

async fn store_bond(state: GlobalState, bond: &BondInformation) {
    let level = match bond.security_level {
        SecurityLevel::NoEncryption => 0,
        SecurityLevel::Encrypted => 1,
        SecurityLevel::EncryptedAuthenticated => 2,
    };
    let record = codec::encode_bond(
        bond.identity.addr.addr.into_inner(),
        bond.ltk.to_le_bytes(),
        level,
        bond.identity.addr.kind == AddrKind::RANDOM,
    );
    if let Err(e) = state.nvs.set(&NVS_BONDING_KEY, record.as_slice()).await {
        log::error!("[ble] storing bond failed ({e:?})");
        crate::log_once!(codes::NVS_BONDING_KEY_WRITE_FAILED);
    }
}

async fn load_bond(state: GlobalState) -> Option<BondInformation> {
    let buf = state.nvs.get::<Vec<u8>>(&NVS_BONDING_KEY).await.ok()?;
    let (addr, ltk, level, random) = codec::decode_bond(&buf)?;
    Some(BondInformation {
        identity: Identity {
            addr: match random {
                true => Address::random(addr),
                false => Address::new(AddrKind::PUBLIC, BdAddr::new(addr)),
            },
            irk: None,
        },
        security_level: match level {
            0 => SecurityLevel::NoEncryption,
            1 => SecurityLevel::Encrypted,
            _ => SecurityLevel::EncryptedAuthenticated,
        },
        is_bonded: true,
        ltk: LongTermKey::from_le_bytes(ltk),
    })
}
