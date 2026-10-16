use core::fmt::Write;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

use crate::{state::GlobalState, utils::buttons::Button};

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

    show(&global_state, "PERIPHERALS QA", result);
}

/// Walks the QA steps; any event other than the expected one fails the run.
async fn qa_inner(global_state: &GlobalState) -> bool {
    let buttons = [Button::First, Button::Second, Button::Third, Button::Fourth];
    for (i, button) in buttons.into_iter().enumerate() {
        let mut line = heapless::String::<16>::new();
        _ = write!(line, "Button {}", i + 1);

        show(global_state, &line, "Down");
        if !matches!(EVENTS.receive().await, QaSignal::ButtonDown(b) if Button::from_mask(b) == Some(button))
        {
            return false;
        }

        show(global_state, &line, "Up");
        if !matches!(EVENTS.receive().await, QaSignal::ButtonUp(b) if Button::from_mask(b) == Some(button))
        {
            return false;
        }
    }

    show(global_state, "Scan card", "1");
    let QaSignal::Rfid(first_card) = EVENTS.receive().await else {
        return false;
    };

    show(global_state, "Scan card", "1 again");
    if EVENTS.receive().await != QaSignal::Rfid(first_card) {
        return false;
    }

    show(global_state, "Scan card", "2");
    if !matches!(EVENTS.receive().await, QaSignal::Rfid(card) if card != first_card) {
        return false;
    }

    // pads builds: touching both pads stands in for connecting the stackmat
    let (connect, solve) = if cfg!(feature = "timer-func") {
        (("Touch both", "pads"), "Pads solve")
    } else {
        (("Connect", "Stackmat"), "Stackmat solve")
    };
    show(global_state, connect.0, connect.1);
    if EVENTS.receive().await != QaSignal::StackmatConnected {
        return false;
    }

    show(global_state, solve, "> 1s");
    if !matches!(EVENTS.receive().await, QaSignal::Stackmat(time) if time > 1000) {
        return false;
    }

    let mut ap_name = heapless::String::<16>::new();
    _ = write!(ap_name, "FKM-{:X}", crate::utils::get_efuse_u32());
    show(global_state, "Setup WIFI", &ap_name);
    EVENTS.receive().await == QaSignal::WifiSetup
}

fn show(global_state: &GlobalState, line1: &str, line2: &str) {
    global_state
        .ui
        .update(|u| u.custom_message = Some((line1.into(), line2.into())));
}

/// Reports an event to the QA flow (never blocks; drops it if 8 are already queued).
pub fn send_qa_resp(val: QaSignal) {
    if EVENTS.try_send(val).is_err() {
        log::warn!("QA event dropped: {val:?}");
    }
}
