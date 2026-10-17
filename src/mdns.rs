use crate::consts::MDNS_RESEND_INTERVAL;
use embassy_net::Stack;
use embassy_time::{Duration, Timer};

const FKMTIME_MDNS: &str = "_fkmtime._tcp.local.";

/// Resolves the `ws` TXT value of the fkmtime service, retrying until found.
pub async fn mdns_query(stack: Stack<'static>) -> heapless::String<255> {
    let mut buf = [0; 1024];
    loop {
        match esp_hal_mdns::resolve(
            stack,
            FKMTIME_MDNS,
            &mut buf,
            Duration::from_millis(MDNS_RESEND_INTERVAL),
            Duration::from_secs(30),
        )
        .await
        {
            Ok(info) => {
                if let Some(ws) = info.txt.get("ws").and_then(|w| w.try_into().ok()) {
                    return ws;
                }
            }
            Err(e) => {
                log::warn!("mdns: {e:?}");
                // a bind error returns immediately; don't spin
                Timer::after(Duration::from_secs(1)).await;
            }
        }
    }
}
