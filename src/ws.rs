use crate::{
    scenes::{GlobalState, Scene},
    structs::{ApiError, FromPacket, TimerPacket, TimerPacketInner},
};
use alloc::string::String;
use core::str::FromStr;
use embassy_net::{
    tcp::{TcpReader, TcpSocket, TcpWriter},
    Stack,
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, pubsub::PubSubChannel,
};
use embassy_time::Timer;
use embedded_io_async::Write;
use esp_wifi::wifi::{WifiDevice, WifiStaDevice};
use ws_framer::{RngProvider, WsFrame, WsFrameOwned, WsRxFramer, WsTxFramer, WsUrl};

static FRAME_CHANNEL: Channel<CriticalSectionRawMutex, WsFrameOwned, 10> = Channel::new();
static TAGGED_RETURN: PubSubChannel<CriticalSectionRawMutex, (u64, TimerPacket), 20, 20, 4> =
    PubSubChannel::new();

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
                ws_reader(&mut reader, &mut rx_framer, global_state.clone()),
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

async fn ws_reader(
    reader: &mut TcpReader<'_>,
    framer: &mut WsRxFramer<'_>,
    global_state: GlobalState,
) -> Result<(), ()> {
    let tagged_publisher = TAGGED_RETURN.publisher().map_err(|_| ())?;

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
                        if let Some(tag) = timer_packet.tag {
                            tagged_publisher.publish((tag, timer_packet)).await;
                            continue;
                        }

                        match timer_packet.data {
                            //TimerPacket::StartUpdate { esp_id, version, build_time, size, firmware } => todo!(),
                            //TimerPacket::SolveConfirm { esp_id, competitor_id, session_id } => todo!(),
                            //TimerPacket::DelegateResponse { esp_id, should_scan_cards, solve_time, penalty } => todo!(),
                            //TimerPacket::ApiError { esp_id, error, should_reset_time } => todo!(),
                            //TimerPacket::DeviceSettings { esp_id, use_inspection, secondary_text, added } => todo!(),
                            //TimerPacket::EpochTime { current_epoch } => todo!(),
                            _ => {}
                        }
                    }
                    Err(e) => {
                        log::error!("timer_packet_fail: {e:?}\nTried to parse:\n{text}\n\n");
                    }
                },
                WsFrame::Binary(_) => todo!(),
                WsFrame::Close(_, _) => todo!(),
                WsFrame::Ping(_) => {
                    FRAME_CHANNEL
                        .send(WsFrameOwned::Pong(alloc::vec::Vec::new()))
                        .await;
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
    let recv = FRAME_CHANNEL.receiver();
    loop {
        let frame = recv.receive().await;
        let data = framer.frame(frame.into_ref());
        writer.write_all(data).await.map_err(|_| ())?;
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
    FRAME_CHANNEL
        .send(WsFrameOwned::Text(serde_json::to_string(&packet).unwrap()))
        .await;
}

pub async fn send_request<T>(packet: TimerPacketInner) -> Result<T, ApiError>
where
    T: FromPacket,
{
    let tag: u64 = HalRandom::random_u32() as u64;
    let packet = TimerPacket {
        tag: Some(tag),
        data: packet,
    };
    send_packet(packet).await;

    // TODO: timeout
    let packet = wait_for_tagged_response(tag).await;
    FromPacket::from_packet(packet)
}

pub async fn send_tagged_packet(packet: TimerPacketInner) -> TimerPacket {
    let tag: u64 = HalRandom::random_u32() as u64;
    let packet = TimerPacket {
        tag: Some(tag),
        data: packet,
    };
    send_packet(packet).await;

    // TODO: timeout
    wait_for_tagged_response(tag).await
}

async fn wait_for_tagged_response(tag: u64) -> TimerPacket {
    let mut subscriber = TAGGED_RETURN.subscriber().unwrap();
    loop {
        let (packet_tag, packet) = subscriber.next_message_pure().await;
        if packet_tag == tag {
            return packet;
        }
    }
}
