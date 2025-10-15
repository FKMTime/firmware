use crate::state::GlobalState;
use core::cell::RefCell;
use embassy_futures::join::join;
use embassy_time::{Duration, Timer};
use esp_radio::{Controller as RadioController, ble::controller::BleConnector};
use rand_core::OsRng;
use trouble_host::prelude::*;

#[embassy_executor::task]
pub async fn bluetooth_timer_task(
    init: &'static RadioController<'static>,
    bt: esp_hal::peripherals::BT<'static>,
    state: GlobalState,
) {
    let Ok(connector) = BleConnector::new(init, bt, esp_radio::ble::Config::default()) else {
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
        mut central,
        mut runner,
        ..
    } = stack.build();

    let mut has_bond_info = if let Some(bond_info) = load_bonding_info(&state.nvs).await {
        log::info!("Bond stored. Adding to stack.");
        stack.add_bond_information(bond_info).unwrap();
        true
    } else {
        log::info!("No bond stored.");
        false
    };

    /*
    let printer = Printer {
        seen: RefCell::new(heapless::Deque::new()),
        state: &state,
    };

    let mut scanner = Scanner::new(central);
    let _ = join(runner.run_with_handler(&printer), async {
        let config = ScanConfig::default();
        let mut _session = scanner.scan(&config).await.unwrap();
        // Scan forever
        loop {
            Timer::after(Duration::from_secs(1)).await;
        }
    })
    .await;
    */

    // let central = scanner.into_inner();
    let display_addr = [156, 158, 110, 52, 62, 236];
    let target: Address = Address::random(display_addr);

    let config = ConnectConfig {
        connect_params: Default::default(),
        scan_config: ScanConfig {
            filter_accept_list: &[(target.kind, &target.addr)],
            ..Default::default()
        },
    };

    log::info!("Scanning for peripheral...");
    let _ = join(runner.run(), async {
        log::info!("Connecting");

        let conn = central.connect(&config).await.unwrap();
        // Allow bonding if a bond isn't already stored
        conn.set_bondable(!has_bond_info).unwrap();
        log::info!("Connected, creating gatt client");

        {
            conn.request_security().unwrap();
            loop {
                match conn.next().await {
                    ConnectionEvent::PairingComplete {
                        security_level,
                        bond,
                    } => {
                        log::info!("Pairing complete: {:?}", security_level);
                        if let Some(bond) = bond {
                            store_bonding_info(&state.nvs, &bond).await;
                            has_bond_info = true;
                        }
                        break;
                    }
                    ConnectionEvent::PairingFailed(err) => {
                        log::error!("Pairing failed: {:?}", err);
                        break;
                    }
                    ConnectionEvent::Disconnected { reason } => {
                        log::error!("Disconnected: {:?}", reason);
                        break;
                    }
                    _ => {}
                }
            }
        }

        let client = GattClient::<_, DefaultPacketPool, 10>::new(&stack, &conn)
            .await
            .unwrap();

        let _ = join(client.task(), async {
            log::info!("Looking for battery service");
            let services = client
                .services_by_uuid(&Uuid::new_short(0x180f))
                .await
                .unwrap();
            let service = services.first().unwrap().clone();

            log::info!("Looking for value handle");
            let c: Characteristic<u8> = client
                .characteristic_by_uuid(&service, &Uuid::new_short(0x2a19))
                .await
                .unwrap();

            log::info!("Subscribing notifications");
            let mut listener = client.subscribe(&c, false).await.unwrap();

            let _ = join(
                async {
                    loop {
                        let mut data = [0; 1];
                        client.read_characteristic(&c, &mut data[..]).await.unwrap();
                        log::info!("Read value: {}", data[0]);
                        Timer::after(Duration::from_secs(10)).await;
                    }
                },
                async {
                    loop {
                        let data = listener.next().await;
                        log::info!(
                            "Got notification: {:?} (val: {})",
                            data.as_ref(),
                            data.as_ref()[0]
                        );
                    }
                },
            )
            .await;
        })
        .await;
    })
    .await;
}

async fn store_bonding_info(nvs: &esp_hal_wifimanager::Nvs, info: &BondInformation) {
    let mut buf = [0; 32];
    _ = nvs.invalidate_key(b"BONDING_INFO").await;

    buf[..6].copy_from_slice(info.identity.bd_addr.raw());
    buf[6..22].copy_from_slice(info.ltk.to_le_bytes().as_slice());
    log::info!(
        "store {:?} {:?} {:?}",
        info.identity.bd_addr,
        info.ltk,
        info.security_level
    );
    buf[22] = match info.security_level {
        SecurityLevel::NoEncryption => 0,
        SecurityLevel::Encrypted => 1,
        SecurityLevel::EncryptedAuthenticated => 2,
    };

    nvs.append_key(b"BOUNDING_KEY", &buf).await.unwrap();
}

async fn load_bonding_info(nvs: &esp_hal_wifimanager::Nvs) -> Option<BondInformation> {
    let mut buf = [0; 32];
    let res = nvs.get_key(b"BOUNDING_KEY", &mut buf).await;
    if res.is_err() {
        return None;
    }

    let bd_addr = BdAddr::new(buf[..6].try_into().unwrap());
    let security_level = match buf[22] {
        0 => SecurityLevel::NoEncryption,
        1 => SecurityLevel::Encrypted,
        2 => SecurityLevel::EncryptedAuthenticated,
        _ => return None,
    };
    let ltk = LongTermKey::from_le_bytes(buf[6..22].try_into().unwrap());

    log::info!("load {:?} {:?} {:?}", bd_addr, ltk, security_level);
    return Some(BondInformation {
        identity: Identity { bd_addr, irk: None },
        security_level,
        is_bonded: true,
        ltk,
    });
}

#[allow(dead_code)]
struct Printer<'a> {
    seen: RefCell<heapless::Deque<BdAddr, 128>>,
    state: &'a GlobalState,
}

impl<'a> EventHandler for Printer<'a> {
    fn on_adv_reports(&self, mut it: LeAdvReportsIter<'_>) {
        let mut seen = self.seen.borrow_mut();
        while let Some(Ok(report)) = it.next() {
            if !seen.iter().any(|b| b.raw() == report.addr.raw()) {
                if let Some(name) = parse_device_name(report.data) {
                    log::debug!("[ble] discovered: {:?} with name: {name}", report.addr);
                    if name.starts_with("FKMD-") {
                        log::info!("Disovered FKM Display! [{:?}] ({name})", report.addr);
                    }
                }

                if seen.is_full() {
                    seen.pop_front();
                }
                seen.push_back(report.addr).unwrap();
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
