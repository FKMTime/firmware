use crate::{
    state::{BleAction, GlobalState, MenuScene},
    structs::BleDisplayDevice,
};
use alloc::string::ToString;
use core::cell::RefCell;
use embassy_futures::select::{Either, select, select3, select4};
use embassy_sync::{
    blocking_mutex::raw::NoopRawMutex,
    channel::{Channel, Sender},
};
use embassy_time::{Duration, Timer, with_timeout};
use esp_radio::{Controller as RadioController, ble::controller::BleConnector};
use rand_core::OsRng;
use trouble_host::prelude::*;

#[embassy_executor::task]
pub async fn bluetooth_timer_task(
    init: &'static RadioController<'static>,
    bt: esp_hal::peripherals::BT<'static>,
    state: GlobalState,
) {
    loop {
        let mut bond_info = if let Some(bond_info) = load_bonding_info(&state.nvs).await {
            log::info!("Bond stored.");
            Some(bond_info)
        } else {
            log::info!("No bond stored.");

            let current_menu_scene = state.state.lock().await.menu_scene.clone();
            if current_menu_scene != Some(MenuScene::BtDisplay) {
                loop {
                    let sig = state.ble_sig.wait().await;
                    if let BleAction::StartScan = sig {
                        break;
                    }
                }
            }

            None
        };

        let Ok(connector) = BleConnector::new(
            init,
            unsafe { bt.clone_unchecked() },
            esp_radio::ble::Config::default(),
        ) else {
            log::error!("Cannot init ble connector");
            return;
        };

        let controller: ExternalController<_, 20> = ExternalController::new(connector);

        let address: Address = Address::random(esp_hal::efuse::Efuse::mac_address());
        log::info!("[ble] address = {address:x?}");

        let mut resources: HostResources<DefaultPacketPool, 1, 3> = HostResources::new();
        let stack = trouble_host::new(controller, &mut resources)
            .set_random_address(address)
            .set_random_generator_seed(&mut OsRng);
        let Host {
            central,
            mut runner,
            ..
        } = stack.build();

        let mut central = match bond_info {
            Some(ref bond) => {
                if let Err(e) = stack.add_bond_information(bond.clone()) {
                    log::error!("Add bond information failed! ({e:?})");
                    break;
                }
                central
            }
            None => {
                let discovery_channel: Channel<NoopRawMutex, BleDisplayDevice, 10> =
                    embassy_sync::channel::Channel::new();
                let printer = BleDiscovery {
                    seen: RefCell::new(heapless::Deque::new()),
                    sender: discovery_channel.sender(),
                };

                let mut scanner = Scanner::new(central);
                {
                    state
                        .state
                        .lock()
                        .await
                        .discovered_bluetooth_devices
                        .clear();
                }

                let _ = select4(
                    runner.run_with_handler(&printer),
                    async {
                        let config = ScanConfig::default();
                        let mut _session = match scanner.scan(&config).await {
                            Ok(s) => s,
                            Err(e) => {
                                log::error!("Cannot start ble scan! ({e:?})");
                                return;
                            }
                        };

                        loop {
                            Timer::after_millis(10000).await;
                        }
                    },
                    async {
                        loop {
                            let recv = discovery_channel.receive().await;
                            {
                                let mut state = state.state.lock().await;
                                if state.selected_bluetooth_item
                                    >= state.discovered_bluetooth_devices.len()
                                    && state.selected_bluetooth_item > 0
                                {
                                    state.selected_bluetooth_item += 1;
                                }

                                state.discovered_bluetooth_devices.push(recv);
                            }
                        }
                    },
                    async {
                        loop {
                            Timer::after_millis(200).await;
                            if state.ble_sig.signaled() {
                                break;
                            }
                        }
                    },
                )
                .await;

                scanner.into_inner()
            }
        };

        let display_addr = match bond_info {
            Some(ref bond_info) => bond_info.identity.bd_addr.into_inner(),
            None => loop {
                let sig = state.ble_sig.wait().await;
                if let BleAction::Connect(d) = sig {
                    break d.addr;
                }
            },
        };
        let target: Address = Address::random(display_addr);

        let config = ConnectConfig {
            connect_params: Default::default(),
            scan_config: ScanConfig {
                filter_accept_list: &[(target.kind, &target.addr)],
                ..Default::default()
            },
        };

        log::info!("Scanning for peripheral...");
        let _ = select3(
            runner.run(),
            async {
                loop {
                    let sig = state.ble_sig.wait().await;
                    if let BleAction::Unpair = sig {
                        break;
                    }
                }
            },
            async {
                'outer: loop {
                    log::info!("Connecting to {:?}", target);
                    let conn = match with_timeout(Duration::from_secs(5), central.connect(&config))
                        .await
                    {
                        Ok(Ok(conn)) => conn,
                        Ok(Err(e)) => {
                            log::error!("Failed to connect: {:?}", e);
                            Timer::after(Duration::from_secs(1)).await;
                            continue;
                        }
                        Err(_) => {
                            log::error!("Timeout connecting");
                            Timer::after(Duration::from_secs(1)).await;
                            continue;
                        }
                    };

                    // Allow bonding if a bond isn't already stored
                    if let Err(e) = conn.set_bondable(bond_info.is_none()) {
                        log::error!("Set bondable failed! ({e:?})");
                        continue;
                    }
                    {
                        if let Err(e) = conn.request_security() {
                            log::error!("Request security failed ({e:?})");
                            continue;
                        }

                        loop {
                            match conn.next().await {
                                ConnectionEvent::PairingComplete {
                                    security_level,
                                    bond,
                                } => {
                                    log::info!("Pairing complete: {:?}", security_level);

                                    if let Some(bond) = bond {
                                        store_bonding_info(&state.nvs, &bond).await;
                                        bond_info = Some(bond);
                                    }

                                    if !security_level.encrypted() {
                                        _ = state.nvs.invalidate_key(b"BONDING_KEY").await;
                                        break 'outer;
                                    }

                                    break;
                                }
                                ConnectionEvent::PairingFailed(err) => {
                                    log::error!("Pairing failed: {:?}", err);
                                    break;
                                }
                                ConnectionEvent::Disconnected { reason } => {
                                    log::error!(
                                        "Disconnected1: {:?} ({:x})",
                                        reason,
                                        reason.into_inner()
                                    );
                                    if reason.into_inner() == 0x05
                                    /* || reason.into_inner() == 0x3e */
                                    {
                                        // auth failed
                                        _ = state.nvs.invalidate_key(b"BONDING_KEY").await;
                                        if let Some(ref bond_info) = bond_info {
                                            _ = stack.remove_bond_information(bond_info.identity);
                                        }
                                        break 'outer;
                                    }
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }

                    let Ok(client) =
                        GattClient::<_, DefaultPacketPool, 10>::new(&stack, &conn).await
                    else {
                        log::error!("Failed to create Gatt client!");
                        continue;
                    };

                    let conn_fut = async {
                        loop {
                            if let ConnectionEvent::Disconnected { reason } = conn.next().await {
                                log::info!("Disconnected2: {:?}", reason);
                                break;
                            }
                        }
                    };

                    let write_fut = async {
                        let services = match with_timeout(
                            Duration::from_secs(5),
                            client.services_by_uuid(&Uuid::from(
                                0xa5bad9f2700a4c3db9e2e58ad262d40eu128,
                            )),
                        )
                        .await
                        {
                            Ok(Ok(conn)) => conn,
                            Ok(Err(e)) => {
                                log::error!("Failed to connect: {:?}", e);
                                Timer::after(Duration::from_secs(1)).await;
                                return;
                            }
                            Err(_) => {
                                log::error!("Timeout connecting");
                                Timer::after(Duration::from_secs(1)).await;
                                return;
                            }
                        };

                        let Some(service) = services.first().cloned() else {
                            log::error!("Cannot find ble service!");
                            return;
                        };

                        let Ok(c) = client
                            .characteristic_by_uuid::<u64>(
                                &service,
                                &Uuid::from(0xa5178cade4e045988053a4a78b9281e2u128),
                            )
                            .await
                        else {
                            log::error!("Cannot find ble characteristic!");
                            return;
                        };

                        let mut data = [0; 8];
                        loop {
                            let ms = state.bt_display_signal.wait().await;
                            data.copy_from_slice(&ms.to_be_bytes());

                            if !conn.is_connected() {
                                break;
                            }

                            if client
                                .write_characteristic_without_response(&c, &data[..])
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    };

                    let gatt_and_conn_events = select(conn_fut, write_fut);
                    match select(client.task(), gatt_and_conn_events).await {
                        Either::Second(Either::First(_)) => {
                            log::info!("Connection event loop finished (disconnected)");
                        }
                        Either::Second(Either::Second(_)) => {
                            log::info!("GATT operations finished");
                        }
                        _ => {}
                    }
                }
            },
        )
        .await;
    }
}

async fn store_bonding_info(nvs: &esp_hal_wifimanager::Nvs, info: &BondInformation) {
    let mut buf = [0; 32];
    _ = nvs.invalidate_key(b"BONDING_KEY").await;

    buf[..6].copy_from_slice(info.identity.bd_addr.raw());
    buf[6..22].copy_from_slice(info.ltk.to_le_bytes().as_slice());
    buf[22] = match info.security_level {
        SecurityLevel::NoEncryption => 0,
        SecurityLevel::Encrypted => 1,
        SecurityLevel::EncryptedAuthenticated => 2,
    };

    let res = nvs.append_key(b"BONDING_KEY", &buf).await;
    if let Err(e) = res {
        log::error!("NVS Bonding key store failed! ({e:?})");
    }
}

async fn load_bonding_info(nvs: &esp_hal_wifimanager::Nvs) -> Option<BondInformation> {
    let mut buf = [0; 32];
    let res = nvs.get_key(b"BONDING_KEY", &mut buf).await;
    if res.is_err() {
        return None;
    }

    let bd_addr = BdAddr::new(buf[..6].try_into().expect(""));
    let security_level = match buf[22] {
        0 => SecurityLevel::NoEncryption,
        1 => SecurityLevel::Encrypted,
        2 => SecurityLevel::EncryptedAuthenticated,
        _ => return None,
    };
    let ltk = LongTermKey::from_le_bytes(buf[6..22].try_into().expect(""));

    Some(BondInformation {
        identity: Identity { bd_addr, irk: None },
        security_level,
        is_bonded: true,
        ltk,
    })
}

#[allow(dead_code)]
struct BleDiscovery<'a> {
    seen: RefCell<heapless::Deque<BdAddr, 128>>,
    sender: Sender<'a, NoopRawMutex, BleDisplayDevice, 10>,
}

impl EventHandler for BleDiscovery<'_> {
    fn on_adv_reports(&self, mut it: LeAdvReportsIter<'_>) {
        let mut seen = self.seen.borrow_mut();
        while let Some(Ok(report)) = it.next() {
            if !seen.iter().any(|b| b.raw() == report.addr.raw()) {
                if let Some(name) = parse_device_name(report.data)
                    && name.starts_with("FKMD-")
                {
                    log::info!("Disovered FKM Display! [{:?}] ({name})", report.addr);

                    _ = self.sender.try_send(BleDisplayDevice {
                        name: name.to_string(),
                        addr: report.addr.into_inner(),
                    });
                }

                if seen.is_full() {
                    seen.pop_front();
                }
                _ = seen.push_back(report.addr);
            }
        }
    }
}

#[allow(dead_code)]
fn parse_device_name(data: &[u8]) -> Option<&str> {
    let mut i = 0;
    while i < data.len() {
        let len = data[i] as usize;
        if len == 0 || i + len >= data.len() {
            break;
        }

        let ad_type = data[i + 1];
        let ad_data = &data[i + 2..i + 1 + len];

        // 0x09 = Complete Local Name, 0x08 = Shortened Local Name
        if ad_type == 0x09 || ad_type == 0x08 {
            return core::str::from_utf8(ad_data).ok();
        }

        i += 1 + len;
    }
    None
}
