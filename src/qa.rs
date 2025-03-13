use alloc::format;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};

use crate::state::GlobalState;

static BACK_SIGNAL: Signal<CriticalSectionRawMutex, QaSignal> = Signal::new();
#[derive(Debug)]
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
    let res = qa_inner(&global_state).await;

    if res {
        global_state.state.lock().await.custom_message =
            Some((format!("PERIPHERALS QA"), format!("PASSED")));
    } else {
        global_state.state.lock().await.custom_message =
            Some((format!("PERIPHERALS QA"), format!("NOT GOOD")));
    }
}

async fn qa_inner(global_state: &GlobalState) -> bool {
    for i in 1..5 {
        global_state.state.lock().await.custom_message =
            Some((format!("Button {i}"), format!("Down")));

        loop {
            let val = BACK_SIGNAL.wait().await;
            if let QaSignal::ButtonDown(bin) = val {
                if bin_to_btn_number(bin) == i {
                    break;
                }
            }

            return false;
        }

        global_state.state.lock().await.custom_message =
            Some((format!("Button {i}"), format!("Up")));

        loop {
            let val = BACK_SIGNAL.wait().await;
            if let QaSignal::ButtonUp(bin) = val {
                if bin_to_btn_number(bin) == i {
                    break;
                }
            }

            return false;
        }
    }

    let first_card_scan;
    global_state.state.lock().await.custom_message = Some((format!("Scan card"), format!("1")));
    loop {
        let val = BACK_SIGNAL.wait().await;
        if let QaSignal::Rfid(card) = val {
            first_card_scan = card;
            break;
        }

        return false;
    }

    global_state.state.lock().await.custom_message =
        Some((format!("Scan card"), format!("1 again")));
    loop {
        let val = BACK_SIGNAL.wait().await;
        if let QaSignal::Rfid(card) = val {
            if card == first_card_scan {
                break;
            }
        }

        return false;
    }

    global_state.state.lock().await.custom_message = Some((format!("Scan card"), format!("2")));
    loop {
        let val = BACK_SIGNAL.wait().await;
        if let QaSignal::Rfid(card) = val {
            if card != first_card_scan {
                break;
            }
        }

        return false;
    }

    global_state.state.lock().await.custom_message =
        Some((format!("Connect"), format!("Stackmat")));
    loop {
        let val = BACK_SIGNAL.wait().await;
        if let QaSignal::StackmatConnected = val {
            break;
        }

        return false;
    }

    global_state.state.lock().await.custom_message =
        Some((format!("Stackmat solve"), format!("> 1s")));
    loop {
        let val = BACK_SIGNAL.wait().await;
        if let QaSignal::Stackmat(time) = val {
            if time > 1000 {
                break;
            }
        }

        return false;
    }

    global_state.state.lock().await.custom_message = Some((
        format!("Setup WIFI"),
        format!("FKM-{:X}", crate::utils::get_efuse_u32()),
    ));
    loop {
        let val = BACK_SIGNAL.wait().await;
        if let QaSignal::WifiSetup = val {
            break;
        }

        return false;
    }

    return true;
}

pub fn send_qa_resp(val: QaSignal) {
    BACK_SIGNAL.signal(val);
}

fn bin_to_btn_number(bin: u8) -> u8 {
    match bin {
        0b00000001 => 1,
        0b00000010 => 2,
        0b00000100 => 3,
        0b00001000 => 4,
        _ => u8::MAX,
    }
}
