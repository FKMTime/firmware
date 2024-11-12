use crate::state::{get_current_epoch, GlobalState, EPOCH_BASE};
use crate::structs::{CardInfoResponsePacket, SolveConfirmPacket};
use adv_shift_registers::wrappers::ShifterPin;
use alloc::string::ToString;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::prelude::*;
use esp_hal::{
    dma::{Dma, DmaRxBuf, DmaTxBuf},
    dma_buffers,
    gpio::AnyPin,
    peripherals::DMA,
    spi::{master::Spi, SpiMode},
};
use esp_hal_mfrc522::consts::UidSize;

#[embassy_executor::task]
pub async fn rfid_task(
    miso: AnyPin,
    mosi: AnyPin,
    sck: AnyPin,
    cs_pin: ShifterPin,
    spi: esp_hal::peripherals::SPI2,
    dma: DMA,
    global_state: GlobalState,
) {
    let dma = Dma::new(dma);
    let dma_chan = dma.channel0;
    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(2048);
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();

    let dma_chan = dma_chan.configure_for_async(false, esp_hal::dma::DmaPriority::Priority0);

    //let cs = Output::new(cs, Level::High);
    let spi = Spi::new(spi, 5.MHz(), SpiMode::Mode0);
    let spi = spi.with_sck(sck).with_miso(miso).with_mosi(mosi);
    let spi = spi.with_dma(dma_chan).with_buffers(dma_rx_buf, dma_tx_buf);

    //esp_hal_mfrc522::MFRC522::new(spi, cs, || esp_hal::time::current_time().ticks());
    let mut mfrc522 = esp_hal_mfrc522::MFRC522::new(spi, cs_pin); // embassy-time feature is enabled,
                                                                  // so no need to pass current_time
                                                                  // function

    _ = mfrc522.pcd_init().await;
    //_ = mfrc522.pcd_selftest().await;
    log::debug!("PCD ver: {:?}", mfrc522.pcd_get_version().await);

    if !mfrc522.pcd_is_init().await {
        log::error!("MFRC522 init failed! Try to power cycle to module!");
    }

    loop {
        if mfrc522.picc_is_new_card_present().await.is_ok() {
            let card = mfrc522.get_card(UidSize::Four).await;
            if let Ok(card) = card {
                let card_uid = card.get_number();
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
                        let mut state = global_state.state.lock().await;
                        if !state.should_skip_other_actions() {
                            match state.scene {
                                crate::state::Scene::WaitingForCompetitor => {
                                    if state.current_competitor.is_none() && resp.can_compete {
                                        state.competitor_display = Some(resp.display);
                                        state.current_competitor = Some(resp.card_id);

                                        if state.solve_time.is_some() {
                                            state.scene = crate::state::Scene::Finished;
                                        } else {
                                            state.scene = crate::state::Scene::CompetitorInfo;
                                        }
                                    }
                                }
                                crate::state::Scene::Finished => {
                                    if state.current_competitor != Some(resp.card_id)
                                        && state.time_confirmed
                                    {
                                        state.current_judge = Some(resp.card_id);
                                    } else if state.current_competitor.is_some()
                                        && state.current_competitor == Some(resp.card_id)
                                        && state.time_confirmed
                                    {
                                        let inspection_time = if state.use_inspection
                                            && state.inspection_start.is_some()
                                            && state.inspection_end.is_some()
                                        {
                                            (state.inspection_end.unwrap()
                                                - state.inspection_start.unwrap())
                                            .as_millis()
                                                as i64
                                        } else {
                                            0
                                        };

                                        let resp = crate::ws::send_request::<SolveConfirmPacket>(
                                            crate::structs::TimerPacketInner::Solve {
                                                solve_time: state.solve_time.unwrap(),
                                                penalty: state.penalty.unwrap_or(0) as i64,
                                                competitor_id: state.current_competitor.unwrap(),
                                                judge_id: state.current_judge.unwrap(),
                                                timestamp: get_current_epoch(),
                                                session_id: "TODO-random-gen-uuid-here".to_string(),
                                                delegate: false,
                                                inspection_time,
                                            },
                                        )
                                        .await;

                                        log::info!("solve_resp: {resp:?}");
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Error ({}): {:?}", e.should_reset_time, e.error);
                    }
                }
            }

            _ = mfrc522.picc_halta().await;
        }

        Timer::after(Duration::from_millis(10)).await;
    }
}
