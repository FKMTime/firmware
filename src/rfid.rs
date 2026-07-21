use crate::consts::RFID_RETRY_INIT_MS;
use crate::state::{GlobalState, MenuScene, current_epoch, sleep_state};
use crate::structs::{CardInfoResponsePacket, SolveConfirmPacket};
use crate::translations::{TranslationKey, get_translation};
use alloc::string::ToString;
use anyhow::{Result, anyhow};
use embassy_time::{Duration, Instant, Timer};
use esp_hal_mfrc522::consts::UidSize;

#[cfg(feature = "v3")]
use esp_hal::time::Rate;
#[cfg(feature = "v3")]
use esp_hal::{
    dma::{DmaRxBuf, DmaTxBuf},
    dma_buffers,
    gpio::AnyPin,
    spi::{Mode, master::Spi},
};

#[embassy_executor::task]
pub async fn rfid_task(
    #[cfg(feature = "v4")] i2c: crate::utils::shared_i2c::SharedI2C,
    #[cfg(feature = "v4")] mut buzzer: esp_hal::ledc::channel::Channel<
        'static,
        esp_hal::ledc::LowSpeed,
    >,
    #[cfg(feature = "v3")] miso: AnyPin<'static>,
    #[cfg(feature = "v3")] mosi: AnyPin<'static>,
    #[cfg(feature = "v3")] sck: AnyPin<'static>,
    #[cfg(feature = "v3")] cs_pin: adv_shift_registers::wrappers::ShifterPin,
    #[cfg(feature = "v3")] spi: esp_hal::peripherals::SPI2<'static>,
    #[cfg(feature = "v3")] dma_chan: esp_hal::peripherals::DMA_CH0<'static>,
    global_state: GlobalState,
) {
    #[cfg(feature = "v4")]
    let mut mfrc522 = {
        let driver = esp_hal_mfrc522::drivers::I2CDriver::new(i2c, 0x28);
        esp_hal_mfrc522::MFRC522::new(driver)
    };

    #[allow(clippy::manual_div_ceil)]
    #[cfg(feature = "v3")]
    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(512);
    #[cfg(feature = "v3")]
    let Ok(dma_tx_buf) = DmaTxBuf::new(tx_descriptors, tx_buffer) else {
        log::error!("Dma tx buf failed");
        crate::utils::error_log::add_error(crate::utils::error_log::codes::RFID_DMA_TX_INIT_FAILED)
            .await;
        return;
    };
    #[cfg(feature = "v3")]
    let Ok(dma_rx_buf) = DmaRxBuf::new(rx_descriptors, rx_buffer) else {
        log::error!("Dma rx buf failed");
        crate::utils::error_log::add_error(crate::utils::error_log::codes::RFID_DMA_RX_INIT_FAILED)
            .await;
        return;
    };

    #[cfg(feature = "v3")]
    let Ok(spi) = Spi::new(
        spi,
        esp_hal::spi::master::Config::default()
            .with_frequency(Rate::from_khz(400))
            .with_mode(Mode::_0),
    )
    .map(|s| {
        s.with_sck(sck)
            .with_miso(miso)
            .with_mosi(mosi)
            .with_dma(dma_chan)
            .with_buffers(dma_rx_buf, dma_tx_buf)
            .into_async()
    }) else {
        log::error!("Rfid task error while creating Spi instance!");
        crate::utils::error_log::add_error(crate::utils::error_log::codes::RFID_SPI_CREATE_FAILED)
            .await;
        return;
    };

    #[cfg(feature = "v3")]
    let mut mfrc522 = {
        let Ok(spi) = embedded_hal_bus::spi::ExclusiveDevice::new(spi, cs_pin, embassy_time::Delay)
        else {
            log::error!("Spi bus init failed (cs set high failed)");
            crate::utils::error_log::add_error(
                crate::utils::error_log::codes::RFID_SPI_BUS_INIT_FAILED,
            )
            .await;
            return;
        };

        esp_hal_mfrc522::MFRC522::new(esp_hal_mfrc522::drivers::SpiDriver::new(spi))
    };

    #[cfg(not(feature = "e2e"))]
    {
        let mut error_logged = false;
        loop {
            _ = mfrc522.pcd_init().await;
            if mfrc522.pcd_is_init().await {
                break;
            }

            log::error!("MFRC522 init failed! Try to power cycle to module! Retrying...");
            if !error_logged {
                crate::utils::error_log::add_error(
                    crate::utils::error_log::codes::RFID_INIT_FAILED,
                )
                .await;
                error_logged = true;
            }
            Timer::after(Duration::from_millis(RFID_RETRY_INIT_MS)).await;
        }
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
                false => {
                    _ = mfrc522.pcd_soft_power_up().await;
                    Timer::after_millis(100).await;
                    let mut error_logged = false;
                    loop {
                        _ = mfrc522.pcd_init().await;
                        if mfrc522.pcd_is_init().await {
                            break;
                        }

                        log::error!(
                            "MFRC522 init failed! Try to power cycle to module! Retrying..."
                        );
                        if !error_logged {
                            crate::utils::error_log::add_error(
                                crate::utils::error_log::codes::RFID_INIT_FAILED,
                            )
                            .await;
                            error_logged = true;
                        }
                        Timer::after(Duration::from_millis(RFID_RETRY_INIT_MS)).await;
                    }
                }
            }
        }

        if rfid_sleep {
            Timer::after(Duration::from_millis(500)).await;
            continue;
        }

        #[cfg(feature = "v4")]
        if global_state.buzzer_sound_test.signaled() {
            global_state.buzzer_sound_test.wait().await;
            beep_card_scan(&mut buzzer, &global_state, true).await;
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
        #[cfg(not(feature = "e2e"))]
        let card_uid = card.get_number();
        log::info!("Card UID: {card_uid}");

        let last_scan_time = (Instant::now().saturating_duration_since(last_card.1)).as_millis();
        let is_server_connected = if cfg!(feature = "qa") {
            true
        } else {
            global_state.state.lock_silent().await.conn.server_connected == Some(true)
        };

        if (last_card.0 == card_uid && last_scan_time < 500) || !is_server_connected {
            log::warn!(
                "Skipping card scan: {last_scan_time:?} {card_uid} {}",
                last_card.0
            );
            continue;
        }
        last_card = (card_uid, Instant::now());
        #[cfg(feature = "v4")]
        beep_card_scan(&mut buzzer, &global_state, false).await;

        #[cfg(feature = "qa")]
        {
            crate::qa::send_qa_resp(crate::qa::QaSignal::Rfid(card_uid));

            _ = mfrc522.picc_halta().await;
            continue;
        }

        #[cfg(not(feature = "e2e"))]
        if !crate::state::trust_server() {
            log::error!("Skipping card scan. Server not trusted!");
            global_state.state.lock().await.msg.error_text =
                Some("Server NOT Trusted!".to_string());
            _ = mfrc522.picc_halta().await;
            continue;
        }

        #[cfg(not(feature = "e2e"))]
        let menu_scene = global_state.state.lock_silent().await.ui.menu_scene.clone();
        #[cfg(not(feature = "e2e"))]
        match menu_scene {
            Some(MenuScene::Signing) => {
                let fkm_token = crate::state::fkm_token();

                let status = mfrc522
                    .pcd_authenticate(
                        esp_hal_mfrc522::consts::PICCCommand::PICC_CMD_MF_AUTH_KEY_A,
                        63,
                        &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
                        &card,
                    )
                    .await;

                let mut key = [0; 6];
                key[..4].copy_from_slice(&fkm_token.to_be_bytes());

                log::debug!("signing auth status: {status:?}");
                if status.is_ok() {
                    let mut buff = [0; 18];

                    buff[..6].copy_from_slice(&key);
                    buff[6] = 0xFF;
                    buff[7] = 0x07;
                    buff[8] = 0x80;
                    buff[9] = 0x69;
                    buff[10..16].copy_from_slice(&key);

                    let res = mfrc522.mifare_write(63, &buff, 16).await;
                    if let Err(e) = res {
                        log::error!("write res: {e:?}");
                        global_state.sign_unsign_progress.signal(false);
                        global_state.state.signal();
                        _ = mfrc522.picc_halta().await;
                        _ = mfrc522.pcd_stop_crypto1().await;
                        continue;
                    }

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
                        global_state.sign_unsign_progress.signal(false);
                        global_state.state.signal();
                        _ = mfrc522.picc_halta().await;
                        _ = mfrc522.pcd_stop_crypto1().await;
                        continue;
                    }

                    let mut buff = [0; 16];
                    buff[..16].clone_from_slice(&card_uid.to_be_bytes());
                    let res = mfrc522.mifare_write(62, &buff, 16).await;
                    if res.is_err() {
                        log::error!("Cannot write secured rfid info!");
                        global_state.sign_unsign_progress.signal(false);
                        global_state.state.signal();
                        _ = mfrc522.picc_halta().await;
                        _ = mfrc522.pcd_stop_crypto1().await;
                        continue;
                    }

                    global_state.sign_unsign_progress.signal(true);
                    global_state.state.signal();
                } else {
                    global_state.sign_unsign_progress.signal(false);
                    global_state.state.signal();
                }

                _ = mfrc522.picc_halta().await;
                _ = mfrc522.pcd_stop_crypto1().await;
                continue;
            }
            Some(MenuScene::Unsigning) => {
                let fkm_token = crate::state::fkm_token();
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

                log::debug!("unsigning auth status: {status:?}");
                if status.is_ok() {
                    let buff = [0; 16];
                    _ = mfrc522.mifare_write(62, &buff, 16).await;

                    let mut buff = [0; 18];
                    buff[..6].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
                    buff[6] = 0xFF;
                    buff[7] = 0x07;
                    buff[8] = 0x80;
                    buff[9] = 0x69;
                    buff[10..16].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);

                    let res = mfrc522.mifare_write(63, &buff, 16).await;
                    if let Err(e) = res {
                        log::error!("cannot unsecure card error: {e:?}");
                        global_state.sign_unsign_progress.signal(false);
                        global_state.state.signal();
                        _ = mfrc522.picc_halta().await;
                        _ = mfrc522.pcd_stop_crypto1().await;
                        continue;
                    }

                    global_state.sign_unsign_progress.signal(true);
                    global_state.state.signal();
                } else {
                    log::error!("unsign auth failed!");
                    global_state.sign_unsign_progress.signal(false);
                    global_state.state.signal();
                }

                _ = mfrc522.picc_halta().await;
                _ = mfrc522.pcd_stop_crypto1().await;
                continue;
            }
            _ => {}
        }

        #[cfg(not(feature = "e2e"))]
        if crate::state::secure_rfid() {
            let fkm_token = crate::state::fkm_token();
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

            let secured_uid = u128::from_be_bytes(buff[..16].try_into().unwrap_or_default());
            if secured_uid != card_uid {
                log::error!("Card is not secure!");
                log::debug!("read: {res:?}, data: {buff:#?}");
                _ = mfrc522.picc_halta().await;
                _ = mfrc522.pcd_stop_crypto1().await;
                continue;
            }
        }

        let is_competitor = {
            let state = global_state.state.lock_silent().await;
            state.solve.current_competitor.is_none()
                || state.solve.current_competitor.unwrap_or_default() == card_uid as u64
        };

        let resp = crate::ws::send_request::<CardInfoResponsePacket>(
            crate::structs::TimerPacketInner::CardInfoRequest {
                card_id: card_uid as u64,
                is_competitor,
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
                state.msg.error_text = Some(e.error);
                if e.should_reset_time {
                    state.reset_solve_state();
                }
            }
        }

        #[cfg(feature = "e2e")]
        crate::ws::send_test_ack(&global_state).await;

        #[cfg(not(feature = "e2e"))]
        {
            _ = mfrc522.picc_halta().await;
            if crate::state::secure_rfid() {
                _ = mfrc522.pcd_stop_crypto1().await;
            }
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

    match state.solve.scene {
        crate::state::Scene::WaitingForCompetitor
            if state.solve.current_competitor.is_none() && resp.can_compete =>
        {
            state.solve.competitor_display = Some(resp.display);
            state.solve.current_competitor = Some(resp.card_id);

            let competitor_locale =
                crate::translations::get_locale_index(&resp.country_iso2.to_lowercase());
            if competitor_locale != crate::translations::current_locale_index() {
                crate::translations::select_locale_idx(competitor_locale, global_state);
            }

            match resp.possible_groups.len() {
                1 => {
                    state.solve.solve_group = Some(resp.possible_groups[0].clone());

                    if state.solve.solve_time.is_some() {
                        state.solve.scene = crate::state::Scene::Finished;
                    } else {
                        state.solve.scene = crate::state::Scene::CompetitorInfo;
                    }
                }
                2.. => {
                    state.solve.possible_groups = resp.possible_groups;
                    state.solve.scene = crate::state::Scene::GroupSelect;
                }
                _ => {
                    state.reset_solve_state();
                    state.msg.error_text =
                        Some(get_translation(TranslationKey::EMPTY_GROUPS_ERROR));
                }
            }
        }
        crate::state::Scene::Finished => {
            if state.solve.current_competitor != Some(resp.card_id) && state.solve.time_confirmed {
                state.solve.current_judge = Some(resp.card_id);
            } else if let Some(current_competitor) = state.solve.current_competitor
                && let Some(current_judge) = state.solve.current_judge
                && state.solve.current_competitor == Some(resp.card_id)
                && state.solve.time_confirmed
            {
                let inspection_time = state
                    .use_inspection()
                    .then_some((state.solve.inspection_end, state.solve.inspection_start))
                    .and_then(|(end, start)| end.zip(start))
                    .map(|(end, start)| (end - start).as_millis() as i64)
                    .unwrap_or(0);

                let session_id = match &state.solve.session_id {
                    Some(sess_id) => sess_id.clone(),
                    None => {
                        let sess_id = uuid::Uuid::new_v4().to_string();
                        state.solve.session_id = Some(sess_id.clone());
                        sess_id
                    }
                };

                let Some(ref solve_group) = state.solve.solve_group else {
                    log::error!("Solve group is none! (How would that happen?)");
                    static LOGGED: core::sync::atomic::AtomicBool =
                        core::sync::atomic::AtomicBool::new(false);
                    if !LOGGED.load(core::sync::atomic::Ordering::Relaxed) {
                        crate::utils::error_log::add_error(
                            crate::utils::error_log::codes::RFID_SOLVE_GROUP_MISSING,
                        )
                        .await;

                        LOGGED.store(true, core::sync::atomic::Ordering::Relaxed);
                    }
                    return Ok(());
                };

                #[cfg(not(feature = "e2e"))]
                if !crate::state::trust_server() {
                    log::error!("Skipping solve send. Server not trusted!");
                    return Ok(());
                }

                // Build the request, then drop the lock before the network
                // round-trip so other tasks aren't parked for its timeout.
                let packet = crate::structs::TimerPacketInner::Solve {
                    solve_time: state
                        .solve
                        .solve_time
                        .ok_or(anyhow!("Solve time is None"))?,
                    penalty: state.solve.penalty.unwrap_or(0) as i64,
                    competitor_id: current_competitor,
                    judge_id: current_judge,
                    timestamp: current_epoch(),
                    session_id,
                    delegate: false,
                    inspection_time,
                    group_id: solve_group.group_id.clone(),
                };
                drop(state);

                let resp = crate::ws::send_request::<SolveConfirmPacket>(packet).await;

                if let Ok(resp) = resp {
                    log::warn!("solve_resp: {resp:?}");
                    let mut state = global_state.state.lock().await;
                    state.reset_solve_state();

                    let words: alloc::vec::Vec<&str> = resp.message.split(' ').collect();
                    if words.len() >= 2 {
                        let first_line = words[..2].join(" ");
                        let second_line = words[2..].join(" ");

                        state.msg.custom_message = Some((first_line, second_line));
                    } else {
                        state.msg.custom_message = Some(("Solve".to_string(), "sent".to_string()));
                    }

                    drop(state);
                    crate::state::SavedGlobalState::clear_saved_global_state(&global_state.nvs)
                        .await;
                    Timer::after_millis(3000).await;

                    {
                        global_state.state.lock().await.msg.custom_message = None;
                    }
                }
            } else if state.solve.current_competitor == Some(resp.card_id)
                && state.solve.current_judge.is_none()
                && state.solve.time_confirmed
            {
                state.msg.custom_message = Some((
                    get_translation(TranslationKey::CARDS_CANNOT_BE_THE_SAME_HEADER),
                    get_translation(TranslationKey::CARDS_CANNOT_BE_THE_SAME_FOOTER),
                ));
                drop(state);
                Timer::after_millis(8000).await;
                global_state.state.lock().await.msg.custom_message = None;
            }
        }
        _ => {}
    }

    Ok(())
}

#[cfg(feature = "v4")]
async fn beep_card_scan(
    buzzer: &mut esp_hal::ledc::channel::Channel<'static, esp_hal::ledc::LowSpeed>,
    global_state: &GlobalState,
    sound_test: bool,
) {
    if global_state.state.lock_silent().await.conn.sound_enabled || sound_test {
        use esp_hal::ledc::channel::ChannelIFace;
        const BEEP_DUTY_PERCENT: u8 = 50;
        const BEEP_DURATION_MS: u64 = 100;

        let volume: f32 = match crate::state::buzzer_volume() {
            0 => 0.0,
            v => 0.55 + (v - 1) as f32 * (0.25 / 23.0),
        };
        let volume = volume * volume * volume * volume;
        _ = buzzer.set_duty((BEEP_DUTY_PERCENT as f32 * volume) as u8);
        Timer::after_millis(BEEP_DURATION_MS).await;
        _ = buzzer.set_duty(0);
    }
}
