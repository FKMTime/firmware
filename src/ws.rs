use crate::{scenes::GlobalState, structs::TimerPacket};
use alloc::string::String;
use core::str::FromStr;
use embassy_net::{
    tcp::{TcpReader, TcpSocket, TcpWriter},
    Stack,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::Timer;
use embedded_io_async::Write;
use esp_wifi::wifi::{WifiDevice, WifiStaDevice};
use ws_framer::{RngProvider, WsFrame, WsRxFramer, WsTxFramer, WsUrl};

static PACKET_CHANNEL: Channel<CriticalSectionRawMutex, TimerPacket, 10> = Channel::new();
static FRAME_CHANNEL: Channel<CriticalSectionRawMutex, WsFrame, 5> = Channel::new();

#[embassy_executor::task]
pub async fn ws_task(
    stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>,
    ws_url: String,
    global_state: GlobalState,
) {
    let ws_url = WsUrl::from_str(&ws_url).unwrap();

    let mut rx_buffer = [0; 8192];
    let mut tx_buffer = [0; 8192];

    let mut ws_rx_buf = alloc::vec![0; 8192];
    let mut ws_tx_buf = alloc::vec![0; 8192];

    loop {
        {
            global_state.state.lock().await.server_connected = Some(false);
        }

        let mut socket = TcpSocket::new(&stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(embassy_time::Duration::from_secs(10)));

        let remote_endpoint = (
            embassy_net::Ipv4Address::from_str(ws_url.ip).unwrap(),
            ws_url.port,
        );
        let r = socket.connect(remote_endpoint).await;
        if let Err(e) = r {
            log::error!("connect error: {:?}", e);
            Timer::after_millis(1000).await;
            continue;
        }

        {
            global_state.state.lock().await.server_connected = Some(true);
        }
        log::info!("connected!");
        let mut tx_framer = WsTxFramer::<HalRandom>::new(true, &mut ws_tx_buf);
        let mut rx_framer = WsRxFramer::new(&mut ws_rx_buf);

        let path = alloc::format!(
            "{}?id={}&ver={}&chip={}&bt={}&firmware={}",
            ws_url.path,
            694202137,
            "3.0",
            "no-chip",
            69420,
            "no-firmware"
        );
        socket
            .write_all(&tx_framer.generate_http_upgrade(ws_url.host, &path, None))
            .await
            .unwrap();
        loop {
            let n = socket.read(rx_framer.mut_buf()).await.unwrap();
            let res = rx_framer.process_http_response(n);

            if let Some(code) = res {
                log::info!("http_resp_code: {code}");
                break;
            }
        }

        let (mut reader, mut writer) = socket.split();
        loop {
            let res = embassy_futures::select::select(
                ws_reader(&mut reader, &mut rx_framer),
                ws_writer(&mut writer, &mut tx_framer),
            )
            .await;

            let res = match res {
                embassy_futures::select::Either::First(res) => res,
                embassy_futures::select::Either::Second(res) => res,
            };

            if res.is_err() {
                log::error!("ws: reader or writer err!");
                Timer::after_millis(1000).await;
                break;
            }
        }
    }
}

async fn ws_reader(reader: &mut TcpReader<'_>, framer: &mut WsRxFramer<'_>) -> Result<(), ()> {
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

        framer.revolve_write_offset(n);
        while let Some(frame) = framer.process_data() {
            //log::warn!("recv_frame: opcode:{}", frame.opcode());

            match frame {
                WsFrame::Text(text) => match serde_json::from_str::<TimerPacket>(text) {
                    Ok(timer_packet) => {
                        log::info!("Timer packet recv: {timer_packet:?}");
                    }
                    Err(e) => {
                        log::error!("timer_packet_fail: {e:?}\nTried to parse:\n{text}\n\n");
                    }
                },
                WsFrame::Binary(_) => todo!(),
                WsFrame::Close(_, _) => todo!(),
                WsFrame::Ping(_) => {
                    FRAME_CHANNEL.send(WsFrame::Pong(&[])).await;
                }
                _ => {}
            }
        }
    }
}

async fn ws_writer(
    writer: &mut TcpWriter<'_>,
    framer: &mut WsTxFramer<'_, HalRandom>,
) -> Result<(), ()> {
    let recv = PACKET_CHANNEL.receiver();
    let frame_recv = FRAME_CHANNEL.receiver();
    loop {
        match embassy_futures::select::select(recv.receive(), frame_recv.receive()).await {
            embassy_futures::select::Either::First(to_send) => {
                log::info!("to_send_packet: {to_send:?}");

                writer
                    .write_all(&framer.text(&serde_json::to_string(&to_send).unwrap()))
                    .await
                    .map_err(|_| ())?;
            }
            embassy_futures::select::Either::Second(to_send) => {
                log::info!("to_send_frame: opcode:{}", to_send.opcode());

                writer
                    .write_all(&framer.frame(to_send))
                    .await
                    .map_err(|_| ())?;
            }
        }

        //update_sig.wait().await;
        //writer.write_all(&framer.pong(&[])).await.map_err(|_| ())?;
        //writer.write_all(&framer.text("{\"logs\": {\"esp_id\": 694202137, \"logs\": [{\"millis\": 69420, \"msg\": \"wowowowowo\"}]}}")).await.map_err(|_| ())?;
        //Timer::after_millis(1000).await;
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

pub async fn send_packet(packet: TimerPacket) {
    PACKET_CHANNEL.send(packet).await;
}
