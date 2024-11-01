use core::str::FromStr;
use embassy_net::{tcp::TcpSocket, Stack};
use embassy_time::{Duration, Timer, WithTimeout};
use embedded_io_async::Write;
use esp_wifi::wifi::{WifiDevice, WifiStaDevice};
use ws_framer::{RngProvider, WsFramer, WsUrl};


#[embassy_executor::task]
pub async fn ws_task(stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>, ws_url: heapless::String<255>) {
    let ws_url = WsUrl::from_str(&ws_url).unwrap();

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    let mut buf = alloc::vec![0; 4096];
    'outer: loop {
        let mut socket = TcpSocket::new(&stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(embassy_time::Duration::from_secs(10)));

        let remote_endpoint = (embassy_net::Ipv4Address::from_str(ws_url.ip).unwrap(), ws_url.port);
        let r = socket.connect(remote_endpoint).await;
        if let Err(e) = r {
            log::error!("connect error: {:?}", e);
            continue;
        }

        log::info!("connected!");
        let mut framer = WsFramer::<HalRandom>::new(true, &mut buf);
        let path = alloc::format!("{}?id={}&ver={}&chip={}&bt={}&firmware={}", ws_url.path, 694202137, "3.0", "no-chip", 69420, "no-firmware");
        socket.write_all(&framer.gen_connect_packet(ws_url.host, &path, None)).await.unwrap();


        let mut buf = [0; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        log::warn!("read: {n}");

        // TODO: split
        //socket.spl
        loop {
            loop {
                let res = socket.read(framer.mut_buf()).with_timeout(Duration::from_millis(1)).await;
                if let Ok(read_n) = res {
                    let read_n = read_n.unwrap();
                    if read_n == 0 {
                        break 'outer;
                    }

                    let res = framer.process_data(read_n);
                    if res.is_some() {
                        log::info!("recv: {res:?}");
                    }
                } else {
                    break;
                }
            }

            socket.write_all(&framer.text("Lorem")).await.unwrap();
            Timer::after_millis(1000).await;
        }
    }
}

pub struct HalRandom;
impl RngProvider for HalRandom {
    fn random_u32() -> u32 {
        unsafe { &*esp_hal::peripherals::RNG::PTR }
            .data()
            .read()
            .bits()
    }
}
