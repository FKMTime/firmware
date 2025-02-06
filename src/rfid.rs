use crate::consts::RFID_RETRY_INIT_MS;
use crate::state::{current_epoch, sleep_state, GlobalState};
use crate::structs::{CardInfoResponsePacket, SolveConfirmPacket};
use crate::translations::get_translation;
use alloc::string::ToString;
use anyhow::{anyhow, Result};
use embassy_time::{Duration, Timer};
use esp_hal::{
    dma::{DmaRxBuf, DmaTxBuf},
    dma_buffers,
    gpio::AnyPin,
    spi::{master::Spi, Mode},
    time::RateExtU32,
};

#[cfg(feature = "esp32")]
use mfrc522_01::consts::UidSize;

#[cfg(feature = "esp32c3")]
use mfrc522_02::consts::UidSize;

#[embassy_executor::task]
pub async fn rfid_task(
    miso: AnyPin,
    mosi: AnyPin,
    sck: AnyPin,
    #[cfg(feature = "esp32c3")] cs_pin: adv_shift_registers::wrappers::ShifterPin,
    #[cfg(feature = "esp32")] cs_pin: esp_hal::gpio::Output<'static>,
    spi: esp_hal::peripherals::SPI2,

    #[cfg(feature = "esp32c3")] dma_chan: esp_hal::dma::DmaChannel0,
    #[cfg(feature = "esp32")] dma_chan: esp_hal::dma::Spi2DmaChannel,

    global_state: GlobalState,
) {
    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(512);
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();

    let spi = Spi::new(
        spi,
        esp_hal::spi::master::Config::default()
            .with_frequency(400.kHz())
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(sck)
    .with_miso(miso)
    .with_mosi(mosi)
    .with_dma(dma_chan)
    .with_buffers(dma_rx_buf, dma_tx_buf)
    .into_async();

    #[cfg(feature = "esp32")]
    let mut mfrc522 =
        mfrc522_01::MFRC522::new(spi, cs_pin, || embassy_time::Instant::now().as_micros());

    #[cfg(feature = "esp32c3")]
    let mut mfrc522 = {
        let spi =
            embedded_hal_bus::spi::ExclusiveDevice::new(spi, cs_pin, embassy_time::Delay).unwrap();
        mfrc522_02::MFRC522::new(spi)
    };

    #[cfg(not(feature = "e2e"))]
    loop {
        _ = mfrc522.pcd_init().await;
        if mfrc522.pcd_is_init().await {
            break;
        }

        log::error!("MFRC522 init failed! Try to power cycle to module! Retrying...");
        Timer::after(Duration::from_millis(RFID_RETRY_INIT_MS)).await;
    }

    #[cfg(not(feature = "e2e"))]
    log::debug!("PCD ver: {:?}", mfrc522.pcd_get_version().await);

    let mut rfid_sleep = false;
    loop {
        Timer::after(Duration::from_millis(10)).await;
        if sleep_state() != rfid_sleep {
            rfid_sleep = sleep_state();

            match rfid_sleep {
                true => _ = mfrc522.pcd_soft_power_down().await,
                false => _ = mfrc522.pcd_soft_power_up().await,
            }
        }

        if rfid_sleep {
            Timer::after(Duration::from_millis(500)).await;
            continue;
        }

        #[cfg(feature = "e2e")]
        if !global_state.e2e.card_scan_sig.signaled() {
            continue;
        }

        #[cfg(feature = "e2e")]
        let card_uid = global_state.e2e.card_scan_sig.wait().await;
        #[cfg(feature = "e2e")]
        log::debug!("[E2E] Card scan: {card_uid}");

        #[cfg(not(feature = "e2e"))]
        if mfrc522.picc_is_new_card_present().await.is_err() {
            continue;
        }

        #[cfg(not(feature = "e2e"))]
        let Ok(card_uid) = mfrc522
            .get_card(UidSize::Four)
            .await
            .map(|c| c.get_number())
        else {
            continue;
        };

        log::info!("Card UID: {card_uid}");
        let resp = crate::ws::send_request::<CardInfoResponsePacket>(
            crate::structs::TimerPacketInner::CardInfoRequest {
                card_id: card_uid as u64,
                attendance_device: None,
            },
        )
        .await;

        match resp {
            Ok(resp) => {
                let res = process_card_info_response(resp, &global_state).await;
                if let Err(e) = res {
                    log::error!("[RFID] Process_card_info_response: {e:?}");
                }
            }
            Err(e) => {
                log::error!(
                    "[RFID] Resp_error: ({}): {:?}",
                    e.should_reset_time,
                    e.error
                );

                let mut state = global_state.state.lock().await;
                state.error_text = Some(e.error);
                if e.should_reset_time {
                    state.reset_solve_state(None).await;
                }
            }
        }

        _ = mfrc522.picc_halta().await;
    }
}

async fn process_card_info_response(
    resp: CardInfoResponsePacket,
    global_state: &GlobalState,
) -> Result<()> {
    let mut state = global_state.state.lock().await;
    if state.should_skip_other_actions() {
        return Ok(());
    }

    match state.scene {
        crate::state::Scene::WaitingForCompetitor => {
            if state.current_competitor.is_none() && resp.can_compete {
                state.competitor_display = Some(resp.display);
                state.current_competitor = Some(resp.card_id);

                match resp.possible_groups.len() {
                    1 => {
                        state.solve_group = Some(resp.possible_groups[0].clone());

                        if state.solve_time.is_some() {
                            state.scene = crate::state::Scene::Finished;
                        } else {
                            state.scene = crate::state::Scene::CompetitorInfo;
                        }
                    }
                    2.. => {
                        state.possible_groups = resp.possible_groups;
                        state.scene = crate::state::Scene::GroupSelect;
                    }
                    _ => {
                        state.error_text = Some(get_translation("NO_USER_GROUPS"));
                    }
                }
            }
        }
        crate::state::Scene::Finished => {
            if state.current_competitor != Some(resp.card_id) && state.time_confirmed {
                state.current_judge = Some(resp.card_id);
            } else if state.current_competitor.is_some()
                && state.current_judge.is_some()
                && state.current_competitor == Some(resp.card_id)
                && state.time_confirmed
            {
                let inspection_time = state
                    .use_inspection()
                    .then_some((state.inspection_end, state.inspection_start))
                    .and_then(|(end, start)| end.zip(start))
                    .map(|(end, start)| (end - start).as_millis() as i64)
                    .unwrap_or(0);

                if state.session_id.is_none() {
                    state.session_id = Some(uuid::Uuid::new_v4().to_string());
                }

                let resp = crate::ws::send_request::<SolveConfirmPacket>(
                    crate::structs::TimerPacketInner::Solve {
                        solve_time: state.solve_time.ok_or(anyhow!("Solve time is None"))?,
                        penalty: state.penalty.unwrap_or(0) as i64,
                        competitor_id: state.current_competitor.unwrap(),
                        judge_id: state.current_judge.unwrap(),
                        timestamp: current_epoch(),
                        session_id: state.session_id.clone().unwrap(),
                        delegate: false,
                        inspection_time,
                        group_id: state
                            .solve_group
                            .clone()
                            .map(|r| r.group_id)
                            .unwrap_or("NOT SELECTED ERROR".to_string()), // TODO: add to this
                                                                          // error handling
                    },
                )
                .await;

                if resp.is_ok() {
                    log::info!("solve_resp: {resp:?}");
                    state.reset_solve_state(Some(&global_state.nvs)).await;
                }
            }
        }
        _ => {}
    }

    Ok(())
}
