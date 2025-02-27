use embassy_net::{
    udp::{PacketMetadata, UdpSocket},
    IpAddress, IpEndpoint, Stack,
};
use embassy_time::{Duration, Timer};
use esp_hal_mdns::MdnsQuery;

use crate::consts::MDNS_RESEND_INTERVAL;

pub async fn mdns_query(stack: Stack<'static>) -> heapless::String<255> {
    let mut rx_buffer = [0; 1024];
    let mut tx_buffer = [0; 1024];
    let mut rx_meta = [PacketMetadata::EMPTY; 16];
    let mut tx_meta = [PacketMetadata::EMPTY; 16];
    let mut sock = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );

    let ip_addr = IpAddress::v4(224, 0, 0, 251);
    let ip_endpoint = IpEndpoint::new(ip_addr, 5353);
    _ = sock.bind(5353);
    _ = stack.join_multicast_group(ip_addr);

    let mut mdns = MdnsQuery::new("_stackmat._tcp.local", MDNS_RESEND_INTERVAL, || {
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_millis()
    });
    let mut data_buf = [0; 1024];

    let tmp;
    loop {
        if let Some(data) = mdns.should_send_mdns_packet() {
            _ = sock.send_to(data, ip_endpoint).await;
        }

        if sock.may_recv() {
            let res = sock.recv_from(&mut data_buf).await;
            if let Ok((n, _endpoint)) = res {
                let resp = mdns.parse_mdns_query(&data_buf[..n], Some("ws"));

                if let Some(value) = resp.2 {
                    tmp = value;
                    break;
                }
            }
        }

        Timer::after(Duration::from_millis(50)).await;
    }
    _ = stack.leave_multicast_group(ip_addr);

    tmp
}
