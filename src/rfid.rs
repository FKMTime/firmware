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

const MAIN_LOOP_POLL_MS: u64 = 10;
const SLEEP_POLL_MS: u64 = 500;
const RESCAN_DEBOUNCE_MS: u64 = 500;

// Secured-card scheme (sector 15): block 62 holds the card UID, block 63 is the
// sector trailer (KeyA | access bits FF 07 80 69 | KeyB). A signed card stores
// the token-derived key + its UID; an unsigned card holds the factory default.
#[cfg(not(feature = "e2e"))]
const SEC_DATA_BLOCK: u8 = 62;
#[cfg(not(feature = "e2e"))]
const SECTOR_TRAILER_BLOCK: u8 = 63;
#[cfg(not(feature = "e2e"))]
const DEFAULT_MIFARE_KEY: [u8; 6] = [0xFF; 6];
#[cfg(not(feature = "e2e"))]
const FKM_ACCESS_BYTES: [u8; 4] = [0xFF, 0x07, 0x80, 0x69];

/// Derive the 6-byte sector key from the fkm token (first 4 bytes = token BE).
#[cfg(not(feature = "e2e"))]
fn token_key(fkm_token: i32) -> [u8; 6] {
    let mut key = [0u8; 6];
    key[..4].copy_from_slice(&fkm_token.to_be_bytes());
    key
}

/// Build the 16-byte sector-15 trailer: KeyA | FF 07 80 69 | KeyB.
#[cfg(not(feature = "e2e"))]
fn sector_trailer(key: &[u8; 6]) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..6].copy_from_slice(key);
    b[6..10].copy_from_slice(&FKM_ACCESS_BYTES);
    b[10..16].copy_from_slice(key);
    b
}

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
    init_pcd_with_retry(&mut mfrc522).await;

    #[cfg(not(feature = "e2e"))]
    log::debug!("PCD ver: {:?}", mfrc522.pcd_get_version().await);

    let mut rfid_sleep = false;
    let mut last_card = (0, Instant::now());
    loop {
        Timer::after(Duration::from_millis(MAIN_LOOP_POLL_MS)).await;
        if sleep_state() != rfid_sleep {
            rfid_sleep = sleep_state();

            match rfid_sleep {
                true => _ = mfrc522.pcd_soft_power_down().await,
                false => {
                    _ = mfrc522.pcd_soft_power_up().await;
                    Timer::after_millis(100).await;
                    init_pcd_with_retry(&mut mfrc522).await;
                }
            }
        }

        if rfid_sleep {
            Timer::after(Duration::from_millis(SLEEP_POLL_MS)).await;
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

        if (last_card.0 == card_uid && last_scan_time < RESCAN_DEBOUNCE_MS) || !is_server_connected
        {
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
        if try_handle_sign_unsign(&mut mfrc522, &global_state, &menu_scene, &card, card_uid).await {
            continue;
        }

        #[cfg(not(feature = "e2e"))]
        if crate::state::secure_rfid() && !verify_secure_card(&mut mfrc522, &card, card_uid).await {
            continue;
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

/// Signal the sign/unsign result to the LCD: progress first, then the render wake.
fn report_sign_progress(global_state: &GlobalState, ok: bool) {
    global_state.sign_unsign_progress.signal(ok);
    global_state.state.signal();
}

/// Release the card: halt + stop crypto1 (both errors ignored, as before).
#[cfg(not(feature = "e2e"))]
async fn deactivate_card<D: esp_hal_mfrc522::MfrcDriver>(
    mfrc522: &mut esp_hal_mfrc522::MFRC522<D>,
) {
    _ = mfrc522.picc_halta().await;
    _ = mfrc522.pcd_stop_crypto1().await;
}

/// Sign one card: write the token key + access bits to the trailer, re-auth with
/// it, then store the card UID. Returns whether every step succeeded.
#[cfg(not(feature = "e2e"))]
async fn handle_signing<D: esp_hal_mfrc522::MfrcDriver>(
    mfrc522: &mut esp_hal_mfrc522::MFRC522<D>,
    card: &esp_hal_mfrc522::consts::Uid,
    card_uid: u128,
) -> bool {
    let fkm_token = crate::state::fkm_token();
    let key = token_key(fkm_token);

    let status = mfrc522
        .pcd_authenticate(
            esp_hal_mfrc522::consts::PICCCommand::PICC_CMD_MF_AUTH_KEY_A,
            SECTOR_TRAILER_BLOCK,
            &DEFAULT_MIFARE_KEY,
            card,
        )
        .await;

    log::debug!("signing auth status: {status:?}");
    if status.is_err() {
        return false;
    }

    if let Err(e) = mfrc522
        .mifare_write(SECTOR_TRAILER_BLOCK, &sector_trailer(&key), 16)
        .await
    {
        log::error!("write res: {e:?}");
        return false;
    }

    if mfrc522
        .pcd_authenticate(
            esp_hal_mfrc522::consts::PICCCommand::PICC_CMD_MF_AUTH_KEY_A,
            SECTOR_TRAILER_BLOCK,
            &key,
            card,
        )
        .await
        .is_err()
    {
        log::error!("Cannot auth card!");
        return false;
    }

    if mfrc522
        .mifare_write(SEC_DATA_BLOCK, &card_uid.to_be_bytes(), 16)
        .await
        .is_err()
    {
        log::error!("Cannot write secured rfid info!");
        return false;
    }

    true
}

/// Unsign one card: auth with the token key, wipe the UID, restore the factory
/// trailer. Returns whether every checked step succeeded.
#[cfg(not(feature = "e2e"))]
async fn handle_unsigning<D: esp_hal_mfrc522::MfrcDriver>(
    mfrc522: &mut esp_hal_mfrc522::MFRC522<D>,
    card: &esp_hal_mfrc522::consts::Uid,
) -> bool {
    let fkm_token = crate::state::fkm_token();
    let key = token_key(fkm_token);

    let status = mfrc522
        .pcd_authenticate(
            esp_hal_mfrc522::consts::PICCCommand::PICC_CMD_MF_AUTH_KEY_A,
            SECTOR_TRAILER_BLOCK,
            &key,
            card,
        )
        .await;

    log::debug!("unsigning auth status: {status:?}");
    if status.is_err() {
        log::error!("unsign auth failed!");
        return false;
    }

    // Intentionally unchecked: clearing the UID block is best-effort.
    _ = mfrc522.mifare_write(SEC_DATA_BLOCK, &[0u8; 16], 16).await;

    if let Err(e) = mfrc522
        .mifare_write(
            SECTOR_TRAILER_BLOCK,
            &sector_trailer(&DEFAULT_MIFARE_KEY),
            16,
        )
        .await
    {
        log::error!("cannot unsecure card error: {e:?}");
        return false;
    }

    true
}

/// If the menu is in a sign/unsign scene, handle it (signal + release the card)
/// and return true so the caller skips the rest of the scan. Sign/unsign only
/// exist on real hardware; e2e injects scans and always runs the solve flow.
#[cfg(not(feature = "e2e"))]
async fn try_handle_sign_unsign<D: esp_hal_mfrc522::MfrcDriver>(
    mfrc522: &mut esp_hal_mfrc522::MFRC522<D>,
    global_state: &GlobalState,
    menu_scene: &Option<MenuScene>,
    card: &esp_hal_mfrc522::consts::Uid,
    card_uid: u128,
) -> bool {
    let ok = match menu_scene {
        Some(MenuScene::Signing) => handle_signing(mfrc522, card, card_uid).await,
        Some(MenuScene::Unsigning) => handle_unsigning(mfrc522, card).await,
        _ => return false,
    };

    report_sign_progress(global_state, ok);
    deactivate_card(mfrc522).await;
    true
}

/// Read-side of the secured-card scheme: auth with the token key, read the
/// stored UID from block 62, and check it matches the scanned card. On success
/// returns true WITHOUT halting (the loop tail stops crypto1 only when
/// secure_rfid()); on any failure deactivates the card and returns false.
#[cfg(not(feature = "e2e"))]
async fn verify_secure_card<D: esp_hal_mfrc522::MfrcDriver>(
    mfrc522: &mut esp_hal_mfrc522::MFRC522<D>,
    card: &esp_hal_mfrc522::consts::Uid,
    card_uid: u128,
) -> bool {
    let key = token_key(crate::state::fkm_token());
    if mfrc522
        .pcd_authenticate(
            esp_hal_mfrc522::consts::PICCCommand::PICC_CMD_MF_AUTH_KEY_A,
            SECTOR_TRAILER_BLOCK,
            &key,
            card,
        )
        .await
        .is_err()
    {
        log::error!("Cannot auth card!");
        deactivate_card(mfrc522).await;
        return false;
    }

    // mifare_read needs buff_size >= 18 (NoRoom otherwise).
    let mut buff = [0u8; 18];
    let mut byte_count = 18;
    let res = mfrc522
        .mifare_read(SEC_DATA_BLOCK, &mut buff, &mut byte_count)
        .await;
    if res.is_err() {
        log::error!("Cannot read secured rfid info!");
        deactivate_card(mfrc522).await;
        return false;
    }

    let secured_uid = u128::from_be_bytes(buff[..16].try_into().unwrap_or_default());
    if secured_uid != card_uid {
        log::error!("Card is not secure!");
        log::debug!("read: {res:?}, data: {buff:#?}");
        deactivate_card(mfrc522).await;
        return false;
    }

    true
}

/// Init the reader, retrying (and logging once per call) until it responds.
/// Shared by the boot init and the wake-from-sleep re-init.
async fn init_pcd_with_retry<D: esp_hal_mfrc522::MfrcDriver>(
    mfrc522: &mut esp_hal_mfrc522::MFRC522<D>,
) {
    let mut error_logged = false;
    loop {
        _ = mfrc522.pcd_init().await;
        if mfrc522.pcd_is_init().await {
            break;
        }

        log::error!("MFRC522 init failed! Try to power cycle to module! Retrying...");
        if !error_logged {
            crate::utils::error_log::add_error(crate::utils::error_log::codes::RFID_INIT_FAILED)
                .await;
            error_logged = true;
        }
        Timer::after(Duration::from_millis(RFID_RETRY_INIT_MS)).await;
    }
}

/// Elapsed inspection time in ms (0 if inspection isn't in use or wasn't started).
fn inspection_time_ms(state: &crate::state::SignaledGlobalStateInner) -> i64 {
    state
        .use_inspection()
        .then_some((state.solve.inspection_end, state.solve.inspection_start))
        .and_then(|(end, start)| end.zip(start))
        .map(|(end, start)| (end - start).as_millis() as i64)
        .unwrap_or(0)
}

/// Split the server confirm message into two LCD lines.
fn split_confirm_message(message: &str) -> (alloc::string::String, alloc::string::String) {
    let words: alloc::vec::Vec<&str> = message.split(' ').collect();
    if words.len() >= 2 {
        (words[..2].join(" "), words[2..].join(" "))
    } else {
        ("Solve".to_string(), "sent".to_string())
    }
}

/// Wait `ms`, then clear the custom message (re-locking with the signaling lock).
async fn clear_custom_message_after(global_state: &GlobalState, ms: u64) {
    Timer::after_millis(ms).await;
    global_state.state.lock().await.msg.custom_message = None;
}

/// Register a competitor from a card-info response: set the competitor, switch
/// to their locale, and advance the scene based on their groups. Takes the
/// already-held guard (never its own lock, to avoid a double render signal).
fn register_competitor(
    state: &mut crate::state::SignaledGlobalStateInner,
    resp: &CardInfoResponsePacket,
    global_state: &GlobalState,
) {
    state.solve.competitor_display = Some(resp.display.clone());
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
            state.solve.possible_groups = resp.possible_groups.clone();
            state.solve.scene = crate::state::Scene::GroupSelect;
        }
        _ => {
            // Reset (restores the default locale) BEFORE setting the error so it
            // renders in the default locale.
            state.reset_solve_state();
            state.msg.error_text = Some(get_translation(TranslationKey::EMPTY_GROUPS_ERROR));
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
            register_competitor(&mut state, &resp, global_state);
        }
        crate::state::Scene::Finished => {
            if state.solve.current_competitor != Some(resp.card_id) && state.solve.time_confirmed {
                state.solve.current_judge = Some(resp.card_id);
            } else if let Some(current_competitor) = state.solve.current_competitor
                && let Some(current_judge) = state.solve.current_judge
                && state.solve.current_competitor == Some(resp.card_id)
                && state.solve.time_confirmed
            {
                let inspection_time = inspection_time_ms(&state);

                let session_id = state
                    .solve
                    .session_id
                    .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
                    .clone();

                let Some(ref solve_group) = state.solve.solve_group else {
                    log::error!("Solve group is none! (How would that happen?)");
                    static LOGGED: core::sync::atomic::AtomicBool =
                        core::sync::atomic::AtomicBool::new(false);
                    crate::utils::error_log::report_once(
                        &LOGGED,
                        crate::utils::error_log::codes::RFID_SOLVE_GROUP_MISSING,
                    )
                    .await;
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

                    state.msg.custom_message = Some(split_confirm_message(&resp.message));

                    drop(state);
                    crate::state::SavedGlobalState::clear_saved_global_state(&global_state.nvs)
                        .await;
                    clear_custom_message_after(global_state, 3000).await;
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
                clear_custom_message_after(global_state, 8000).await;
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
