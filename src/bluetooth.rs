use core::cell::RefCell;
use embassy_futures::join::join;
use embassy_time::{Duration, Timer};
use esp_radio::{Controller as RadioController, ble::controller::BleConnector};
use trouble_host::prelude::*;

#[embassy_executor::task]
pub async fn bluetooth_timer_task(
    init: &'static RadioController<'static>,
    bt: esp_hal::peripherals::BT<'static>,
) {
    let Ok(connector) = BleConnector::new(init, bt, esp_radio::ble::Config::default()) else {
        log::error!("Cannot init ble connector");
        return;
    };

    let controller: ExternalController<_, 20> = ExternalController::new(connector);

    let address: Address = Address::random(esp_hal::efuse::Efuse::mac_address());
    log::info!("[ble] address = {address:x?}");

    let mut resources: HostResources<DefaultPacketPool, 1, 3> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).set_random_address(address);
    let Host {
        central,
        mut runner,
        ..
    } = stack.build();

    let printer = Printer {
        seen: RefCell::new(heapless::Deque::new()),
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

    // let central = scanner.into_inner();
}

#[allow(dead_code)]
struct Printer {
    seen: RefCell<heapless::Deque<BdAddr, 128>>,
}

impl EventHandler for Printer {
    fn on_adv_reports(&self, mut it: LeAdvReportsIter<'_>) {
        let mut seen = self.seen.borrow_mut();
        while let Some(Ok(report)) = it.next() {
            if !seen.iter().any(|b| b.raw() == report.addr.raw()) {
                log::info!(
                    "[ble] discovered: {:?} with name: {:?}",
                    report.addr,
                    parse_device_name(report.data)
                );
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
