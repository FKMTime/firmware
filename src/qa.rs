use core::fmt::Write;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

use crate::state::GlobalState;

/// Peripheral events in arrival order (a `Signal` would drop all but the latest).
static EVENTS: Channel<CriticalSectionRawMutex, QaSignal, 8> = Channel::new();

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QaSignal {
    ButtonDown(u8),
    ButtonUp(u8),
    Rfid(u128),
    StackmatConnected,
    Stackmat(u64),
    WifiSetup,
}

#[embassy_executor::task]
pub async fn qa_processor(global_state: GlobalState) {
    let result = if qa_inner(&global_state).await {
        "PASSED"
    } else {
        "NOT GOOD"
    };

    show(&global_state, "PERIPHERALS QA", result).await;
}

/// Walks the QA steps; any event other than the expected one fails the run.
async fn qa_inner(global_state: &GlobalState) -> bool {
    for i in 1..5u8 {
        let mut line = heapless::String::<16>::new();
        _ = write!(line, "Button {i}");

        show(global_state, &line, "Down").await;
        if !matches!(EVENTS.receive().await, QaSignal::ButtonDown(b) if btn_number(b) == Some(i)) {
            return false;
        }

        show(global_state, &line, "Up").await;
        if !matches!(EVENTS.receive().await, QaSignal::ButtonUp(b) if btn_number(b) == Some(i)) {
            return false;
        }
    }

    show(global_state, "Scan card", "1").await;
    let QaSignal::Rfid(first_card) = EVENTS.receive().await else {
        return false;
    };

    show(global_state, "Scan card", "1 again").await;
    if EVENTS.receive().await != QaSignal::Rfid(first_card) {
        return false;
    }

    show(global_state, "Scan card", "2").await;
    if !matches!(EVENTS.receive().await, QaSignal::Rfid(card) if card != first_card) {
        return false;
    }

    show(global_state, "Connect", "Stackmat").await;
    if EVENTS.receive().await != QaSignal::StackmatConnected {
        return false;
    }

    show(global_state, "Stackmat solve", "> 1s").await;
    if !matches!(EVENTS.receive().await, QaSignal::Stackmat(time) if time > 1000) {
        return false;
    }

    let mut ap_name = heapless::String::<16>::new();
    _ = write!(ap_name, "FKM-{:X}", crate::utils::get_efuse_u32());
    show(global_state, "Setup WIFI", &ap_name).await;
    EVENTS.receive().await == QaSignal::WifiSetup
}

async fn show(global_state: &GlobalState, line1: &str, line2: &str) {
    global_state.state.lock().await.custom_message = Some((line1.into(), line2.into()));
}

/// Reports an event to the QA flow (never blocks; drops it if 8 are already queued).
pub fn send_qa_resp(val: QaSignal) {
    if EVENTS.try_send(val).is_err() {
        log::warn!("QA event dropped: {val:?}");
    }
}

fn btn_number(mask: u8) -> Option<u8> {
    match mask {
        0b0001 => Some(1),
        0b0010 => Some(2),
        0b0100 => Some(3),
        0b1000 => Some(4),
        _ => None,
    }
}
