use core::str::FromStr;
use alloc::rc::Rc;
use embassy_net::{tcp::{TcpReader, TcpSocket, TcpWriter}, Stack};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::Timer;
use embedded_io_async::Write;
use esp_wifi::wifi::{WifiDevice, WifiStaDevice};
use ws_framer::{RngProvider, WsRxFramer, WsTxFramer, WsUrl};
use crate::scenes::GlobalState;

#[embassy_executor::task]
pub async fn ws_task(stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>, ws_url: heapless::String<255>, global_state: GlobalState) {
    let ws_url = WsUrl::from_str(&ws_url).unwrap();

    let mut rx_buffer = [0; 8192];
    let mut tx_buffer = [0; 8192];

    let mut ws_rx_buf = alloc::vec![0; 8192];
    let mut ws_tx_buf = alloc::vec![0; 8192];

    loop {
        {
            global_state.lock().await.server_connected = Some(false);
        }

        let mut socket = TcpSocket::new(&stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(embassy_time::Duration::from_secs(10)));

        let remote_endpoint = (embassy_net::Ipv4Address::from_str(ws_url.ip).unwrap(), ws_url.port);
        let r = socket.connect(remote_endpoint).await;
        if let Err(e) = r {
            log::error!("connect error: {:?}", e);
            Timer::after_millis(1000).await;
            continue;
        }

        {
            global_state.lock().await.server_connected = Some(true);
        }
        log::info!("connected!");
        let mut tx_framer = WsTxFramer::<HalRandom>::new(true, &mut ws_tx_buf);
        let mut rx_framer = WsRxFramer::new(&mut ws_rx_buf);

        let path = alloc::format!("{}?id={}&ver={}&chip={}&bt={}&firmware={}", ws_url.path, 694202137, "3.0", "no-chip", 69420, "no-firmware");
        socket.write_all(&tx_framer.generate_http_upgrade(ws_url.host, &path, None)).await.unwrap();
        loop {
            let n = socket.read(rx_framer.mut_buf()).await.unwrap();
            let res = rx_framer.process_http_response(n);

            if let Some(code) = res {
                log::info!("http_resp_code: {code}");
                break;
            }
        }


        let (mut reader, mut writer) = socket.split();
        let update_sig = Rc::new(Signal::new());
        loop {
            let res = embassy_futures::select::select(
                ws_reader(&mut reader, &mut rx_framer, update_sig.clone()), 
                ws_writer(&mut writer, &mut tx_framer, update_sig.clone())
            ).await;

            let res = match res {
                embassy_futures::select::Either::First(res) => res,
                embassy_futures::select::Either::Second(res) => res
            };

            if res.is_err() {
                log::error!("ws: reader or writer err!");
                Timer::after_millis(1000).await;
                break;
            }
        }
    }
}

async fn ws_reader(reader: &mut TcpReader<'_>, framer: &mut WsRxFramer<'_>, update_sig: Rc<Signal<NoopRawMutex, ()>>) -> Result<(), ()> {
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
            log::info!("recv_frame: {:?}", frame.opcode());
            update_sig.signal(());
        }
    }
}

async fn ws_writer(writer: &mut TcpWriter<'_>, framer: &mut WsTxFramer<'_, HalRandom>, update_sig: Rc<Signal<NoopRawMutex, ()>>) -> Result<(), ()> {
    loop {
        //update_sig.wait().await;
        //writer.write_all(&framer.pong(&[])).await.map_err(|_| ())?;
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
