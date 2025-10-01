use crate::consts::RFID_RETRY_INIT_MS;
use crate::state::{GlobalState, current_epoch, sleep_state};
use crate::structs::{CardInfoResponsePacket, SolveConfirmPacket};
use crate::translations::{TranslationKey, get_translation};
use alloc::string::ToString;
use anyhow::{Result, anyhow};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::time::Rate;
use esp_hal::{
    dma::{DmaRxBuf, DmaTxBuf},
    dma_buffers,
    gpio::AnyPin,
    spi::{Mode, master::Spi},
};
use esp_hal_mfrc522::consts::UidSize;

#[embassy_executor::task]
pub async fn rfid_task(
    miso: AnyPin<'static>,
    mosi: AnyPin<'static>,
    sck: AnyPin<'static>,
    cs_pin: adv_shift_registers::wrappers::ShifterPin,
    spi: esp_hal::peripherals::SPI2<'static>,
    dma_chan: esp_hal::peripherals::DMA_CH0<'static>,
    global_state: GlobalState,
) {
    #[allow(clippy::manual_div_ceil)]
    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(512);
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).expect("Dma tx buf failed");
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).expect("Dma rx buf failed");

    let spi = Spi::new(
        spi,
        esp_hal::spi::master::Config::default()
            .with_frequency(Rate::from_khz(400))
            .with_mode(Mode::_0),
    )
    .expect("Spi init failed")
    .with_sck(sck)
    .with_miso(miso)
    .with_mosi(mosi)
    .with_dma(dma_chan)
    .with_buffers(dma_rx_buf, dma_tx_buf)
    .into_async();

    let mut mfrc522 = {
        let spi = embedded_hal_bus::spi::ExclusiveDevice::new(spi, cs_pin, embassy_time::Delay)
            .expect("Spi bus init failed (cs set high failed)");

        esp_hal_mfrc522::MFRC522::new(esp_hal_mfrc522::drivers::SpiDriver::new(spi))
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
    let mut last_card = (0, Instant::now());
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
        let Ok(card) = mfrc522.get_card(UidSize::Four).await else {
            continue;
        };
        let card_uid = card.get_number();
        log::info!("Card UID: {card_uid}");

        let last_scan_time = (Instant::now().saturating_duration_since(last_card.1)).as_millis();
        if last_card.0 == card_uid && last_scan_time < 500 {
            log::warn!(
                "Skipping card scan: {last_scan_time:?} {card_uid} {}",
                last_card.0
            );
            continue;
        }
        last_card = (card_uid, Instant::now());

        #[cfg(feature = "qa")]
        {
            crate::qa::send_qa_resp(crate::qa::QaSignal::Rfid(card_uid));

            _ = mfrc522.picc_halta().await;
            continue;
        }

        if unsafe { !crate::state::TRUST_SERVER } {
            log::error!("Skipping card scan. Server not trusted!");
            _ = mfrc522.picc_halta().await;
            continue;
        }

        if unsafe { crate::state::SECURE_RFID } {
            let fkm_token = unsafe { crate::state::FKM_TOKEN };
            let mut key = [0; 6];
            key[..4].copy_from_slice(&fkm_token.to_be_bytes());

            let status = mfrc522
                .pcd_authenticate(
                    esp_hal_mfrc522::consts::PICCCommand::PICC_CMD_MF_AUTH_KEY_A,
                    63,
                    &key,
                    &card,
                )
                .await;

            if status.is_err() {
                log::error!("Cannot auth card!");
                _ = mfrc522.picc_halta().await;
                _ = mfrc522.pcd_stop_crypto1().await;
                continue;
            }

            let mut buff = [0; 18];
            let mut byte_count = 18;
            let res = mfrc522.mifare_read(62, &mut buff, &mut byte_count).await;
            if res.is_err() {
                log::error!("Cannot read secured rfid info!");
                _ = mfrc522.picc_halta().await;
                _ = mfrc522.pcd_stop_crypto1().await;
                continue;
            }

            let secured_uid = u128::from_be_bytes(buff[..16].try_into().expect(""));
            if secured_uid != card_uid {
                log::error!("Card is not secure!");
                log::debug!("read: {res:?}, data: {buff:#?}");
                _ = mfrc522.picc_halta().await;
                _ = mfrc522.pcd_stop_crypto1().await;
                continue;
            }
        }

        // TODO: WRITE SECURED
        /*
        let status = mfrc522
            .pcd_authenticate(
                esp_hal_mfrc522::consts::PICCCommand::PICC_CMD_MF_AUTH_KEY_A,
                63,
                &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
                &card,
            )
            .await;
        log::info!("auth status: {status:?}");
        if status.is_ok() {
            let mut buff = [0; 18];
            let mut byte_count = 18;
            let res = mfrc522.mifare_read(63, &mut buff, &mut byte_count).await;
            log::info!("read: {res:?}, data: {buff:#?}");

            let mut custom_key = [0x69, 0x42, 0x00, 0x13, 0x56, 0xAE];

            buff[..6].copy_from_slice(&custom_key);
            buff[6] = 0xFF;
            buff[7] = 0x07;
            buff[8] = 0x80;
            buff[9] = 0x69;
            buff[10..16].copy_from_slice(&custom_key);

            let res = mfrc522.mifare_write(63, &buff, 16).await;
            log::info!("write res: {res:?}");
        }
        */

        let resp = crate::ws::send_request::<CardInfoResponsePacket>(
            crate::structs::TimerPacketInner::CardInfoRequest {
                card_id: card_uid as u64,
                attendance_device: None,
                sign_key: unsafe { crate::state::SIGN_KEY },
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

        #[cfg(feature = "e2e")]
        crate::ws::send_test_ack(&global_state).await;

        #[cfg(not(feature = "e2e"))]
        {
            _ = mfrc522.picc_halta().await;
            //_ = mfrc522.pcd_stop_crypto1().await;
        }
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

                let competitor_locale =
                    crate::translations::get_locale_index(&resp.country_iso2.to_lowercase());
                if competitor_locale != crate::translations::current_locale_index() {
                    crate::translations::select_locale_idx(competitor_locale, global_state);
                }

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
                        state.reset_solve_state(None).await;
                        state.error_text =
                            Some(get_translation(TranslationKey::EMPTY_GROUPS_ERROR));
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

                if state.solve_group.is_none() {
                    log::error!("Solve group is none! (How would that happen?)");
                    return Ok(());
                }

                if unsafe { !crate::state::TRUST_SERVER } {
                    log::error!("Skipping solve send. Server not trusted!");
                    return Ok(());
                }

                let resp = crate::ws::send_request::<SolveConfirmPacket>(
                    crate::structs::TimerPacketInner::Solve {
                        solve_time: state.solve_time.ok_or(anyhow!("Solve time is None"))?,
                        penalty: state.penalty.unwrap_or(0) as i64,
                        competitor_id: state.current_competitor.expect(""),
                        judge_id: state.current_judge.expect(""),
                        timestamp: current_epoch(),
                        session_id: state.session_id.clone().expect(""),
                        delegate: false,
                        inspection_time,
                        group_id: state.solve_group.clone().map(|r| r.group_id).expect(""),
                        sign_key: unsafe { crate::state::SIGN_KEY },
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
