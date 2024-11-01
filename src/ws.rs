use core::str::FromStr;
use embassy_net::{tcp::{TcpReader, TcpSocket, TcpWriter}, Stack};
use embassy_time::{Duration, Timer, WithTimeout};
use embedded_io_async::Write;
use esp_wifi::wifi::{WifiDevice, WifiStaDevice};
use ws_framer::{RngProvider, WsFramer, WsUrl};


#[embassy_executor::task]
pub async fn ws_task(stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>, ws_url: heapless::String<255>) {
    let ws_url = WsUrl::from_str(&ws_url).unwrap();

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    let mut ws_rx_buf = alloc::vec![0; 4096];
    let mut ws_tx_buf = alloc::vec![0; 4096];

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
        let mut tx_framer = WsFramer::<HalRandom>::new(true, &mut ws_tx_buf);
        let mut rx_framer = WsFramer::<HalRandom>::new(false, &mut ws_rx_buf);

        let path = alloc::format!("{}?id={}&ver={}&chip={}&bt={}&firmware={}", ws_url.path, 694202137, "3.0", "no-chip", 69420, "no-firmware");
        socket.write_all(&tx_framer.gen_connect_packet(ws_url.host, &path, None)).await.unwrap();


        let mut buf = [0; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        log::warn!("read: {n}");

        let (mut reader, mut writer) = socket.split();
        loop {
            let res = embassy_futures::select::select(
                ws_reader(&mut reader, &mut rx_framer), 
                ws_writer(&mut writer, &mut tx_framer)
            ).await;

            let res = match res {
                embassy_futures::select::Either::First(res) => res,
                embassy_futures::select::Either::Second(res) => res
            };

            if res.is_err() {
                log::error!("ws: reader or writer err!");
                break;
            }
        }
    }
}

async fn ws_reader(reader: &mut TcpReader<'_>, framer: &mut WsFramer<'_, HalRandom>) -> Result<(), ()> {
    loop {
        let res = reader.read(framer.mut_buf()).await;
        if let Err(e) = res {
            log::error!("ws_read: {e:?}");
            return Err(());
        }

        let n = res.unwrap();
        if n == 0 {
            log::warn!("read_n: 0");
            return Err(());
        }

        if let Some(frame) = framer.process_data(n) {
            log::info!("recv_frame: {:?}", frame);
        }
    }
}

async fn ws_writer(writer: &mut TcpWriter<'_>, framer: &mut WsFramer<'_, HalRandom>) -> Result<(), ()> {
    loop {
        writer.write_all(&framer.text("{\"logs\": {\"esp_id\": 694202137, \"logs\": [{\"millis\": 69420, \"msg\": \"wowowowowo\"}]}}")).await.map_err(|_| ())?;
        Timer::after_millis(1000).await;
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
