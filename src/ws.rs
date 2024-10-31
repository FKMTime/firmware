use embassy_net::{tcp::TcpSocket, Stack};
use embassy_time::Timer;
use embedded_io_async::Write;
use esp_wifi::wifi::{WifiDevice, WifiStaDevice};
use ws_framer::{RngProvider, WsFramer};


#[embassy_executor::task]
pub async fn ws_task(stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>) {
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    let mut buf = alloc::vec![0; 4096];
    loop {
        let mut socket = TcpSocket::new(&stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(embassy_time::Duration::from_secs(10)));
        let remote_endpoint = (embassy_net::Ipv4Address::new(192, 168, 1, 38), 4321);
        let r = socket.connect(remote_endpoint).await;
        if let Err(e) = r {
            log::error!("connect error: {:?}", e);
            continue;
        }

        log::info!("connected!");
        let mut framer = WsFramer::<HalRandom>::new(true, &mut buf);
        socket.write_all(&framer.gen_connect_packet("192.168.1.38:4321", "/", None)).await.unwrap();


        let mut buf = [0; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        log::warn!("read: {n}");

        loop {
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

/*
    let mut buf = vec![0; 10240];
    let mut framer = WsFramer::<StdRandom>::new(true, &mut buf);

    let mut client = TcpStream::connect(ip)?;
    client.write_all(&framer.gen_connect_packet(ip, "/", None))?;

    let mut buf = [0; 1024];
    let n = client.read(&mut buf)?;
    println!("resp_n: {n}");
    println!("buf: {:?}", core::str::from_utf8(&buf[..n]));

    let frame = framer.text("Lorem");
    println!("{:?}", frame);
    client.write_all(&frame)?;
    /*
    let frame = WsMessage::Text("Lorem".to_string())
        .to_data(true, Some(&mut || rand::thread_rng().next_u32()));
    client.write_all(&frame.0[..frame.1])?;
    */

    std::thread::sleep(std::time::Duration::from_secs(1));
    client.write_all(&framer.close(1000))?;

*/
