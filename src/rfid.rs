use crate::{
    consts::RFID_RETRY_INIT_MS,
    state::{GlobalState, Scene, current_epoch, sleep_state},
    structs::{CardInfoResponsePacket, SolveConfirmPacket},
    translations::{TranslationKey, get_translation},
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal_mfrc522::{Interface, Mfrc522};

#[cfg(not(feature = "e2e"))]
use crate::state::MenuScene;
#[cfg(not(feature = "e2e"))]
use esp_hal_mfrc522::{Key, KeyType, Uid};

#[cfg(feature = "v3")]
use esp_hal::time::Rate;
#[cfg(feature = "v3")]
use esp_hal::{
    dma_rx_buffer, dma_tx_buffer,
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
        let driver = esp_hal_mfrc522::I2cInterface::new(i2c, 0x28);
        esp_hal_mfrc522::Mfrc522::new(driver)
    };

    #[cfg(feature = "v3")]
    let Ok(dma_tx_buf) = dma_tx_buffer!(512) else {
        log::error!("Dma tx buf failed");
        crate::utils::error_log::add_error(crate::utils::error_log::codes::RFID_DMA_TX_INIT_FAILED);
        return;
    };
    #[cfg(feature = "v3")]
    let Ok(dma_rx_buf) = dma_rx_buffer!(512) else {
        log::error!("Dma rx buf failed");
        crate::utils::error_log::add_error(crate::utils::error_log::codes::RFID_DMA_RX_INIT_FAILED);
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
        crate::utils::error_log::add_error(crate::utils::error_log::codes::RFID_SPI_CREATE_FAILED);
        return;
    };

    #[cfg(feature = "v3")]
    let mut mfrc522 = {
        let Ok(spi) = embedded_hal_bus::spi::ExclusiveDevice::new(spi, cs_pin, embassy_time::Delay)
        else {
            log::error!("Spi bus init failed (cs set high failed)");
            crate::utils::error_log::add_error(
                crate::utils::error_log::codes::RFID_SPI_BUS_INIT_FAILED,
            );
            return;
        };

        esp_hal_mfrc522::Mfrc522::new(esp_hal_mfrc522::SpiInterface::new(spi))
    };

    #[cfg(not(feature = "e2e"))]
    init_until_ready(&mut mfrc522).await;

    #[cfg(not(feature = "e2e"))]
    log::debug!("PCD ver: {:?}", mfrc522.version().await);

    let mut rfid_sleep = false;
    let mut last_card = (0, Instant::now());
    loop {
        Timer::after(Duration::from_millis(10)).await;
        if sleep_state() != rfid_sleep {
            rfid_sleep = sleep_state();

            if rfid_sleep {
                _ = mfrc522.power_down().await;
            } else {
                _ = mfrc522.power_up().await;
                Timer::after_millis(100).await;
                init_until_ready(&mut mfrc522).await;
            }
        }

        if rfid_sleep {
            Timer::after(Duration::from_millis(500)).await;
            continue;
        }

        #[cfg(feature = "v4")]
        if global_state.buzzer_sound_test.try_take().is_some() {
            beep_card_scan(&mut buzzer, &global_state, true).await;
            continue;
        }

        #[cfg(feature = "e2e")]
        let Some(card_uid) = global_state.e2e.card_scan_sig.try_take() else {
            continue;
        };
        #[cfg(feature = "e2e")]
        log::debug!("[E2E] Card scan: {card_uid}");

        #[cfg(not(feature = "e2e"))]
        if !matches!(mfrc522.is_new_card_present().await, Ok(true)) {
            continue;
        }

        #[cfg(not(feature = "e2e"))]
        let Ok(card) = mfrc522.select().await else {
            continue;
        };
        #[cfg(not(feature = "e2e"))]
        let card_uid = card.to_u128();
        log::info!("Card UID: {card_uid}");

        let last_scan_time = Instant::now()
            .saturating_duration_since(last_card.1)
            .as_millis();
        let is_server_connected =
            cfg!(feature = "qa") || global_state.state.value().await.server_connected == Some(true);

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

        // qa only checks that cards are read
        if cfg!(feature = "qa") {
            #[cfg(feature = "qa")]
            crate::qa::send_qa_resp(crate::qa::QaSignal::Rfid(card_uid));
            _ = mfrc522.halt().await;
            continue;
        }

        handle_card(
            &mut mfrc522,
            #[cfg(not(feature = "e2e"))]
            &card,
            card_uid,
            &global_state,
        )
        .await;
    }
}

/// Re-initializes the reader until it answers (logs the failure once).
async fn init_until_ready<I: Interface>(mfrc522: &mut Mfrc522<I>) {
    let mut error_logged = false;
    loop {
        _ = mfrc522.init().await;
        if matches!(mfrc522.is_initialized().await, Ok(true)) {
            return;
        }

        log::error!("MFRC522 init failed! Try to power cycle to module! Retrying...");
        if !error_logged {
            crate::utils::error_log::add_error(crate::utils::error_log::codes::RFID_INIT_FAILED);
            error_logged = true;
        }
        Timer::after(Duration::from_millis(RFID_RETRY_INIT_MS)).await;
    }
}

/// Card key derived from the server token: token bytes, then two zero bytes.
#[cfg(not(feature = "e2e"))]
fn token_key() -> [u8; 6] {
    let mut key = [0; 6];
    key[..4].copy_from_slice(&crate::state::fkm_token().to_be_bytes());
    key
}

/// Sector trailer with `key` as key A and B and the default access bits.
#[cfg(not(feature = "e2e"))]
fn sector_trailer(key: &[u8; 6]) -> [u8; 16] {
    let mut buff = [0u8; 16];
    buff[..6].copy_from_slice(key);
    buff[6..10].copy_from_slice(&[0xFF, 0x07, 0x80, 0x69]);
    buff[10..].copy_from_slice(key);
    buff
}

/// Locks sector 15 with the token key and stores the uid in block 62.
#[cfg(not(feature = "e2e"))]
async fn sign_card<I: Interface>(
    mfrc522: &mut Mfrc522<I>,
    card: &Uid,
    card_uid: u128,
) -> Result<(), &'static str> {
    let key = token_key();
    mfrc522
        .authenticate(KeyType::A, 63, &Key::DEFAULT, card)
        .await
        .map_err(|_| "default key auth failed")?;
    mfrc522
        .write_block(63, &sector_trailer(&key))
        .await
        .map_err(|_| "trailer write failed")?;
    mfrc522
        .authenticate(KeyType::A, 63, &Key(key), card)
        .await
        .map_err(|_| "token key auth failed")?;
    mfrc522
        .write_block(62, &card_uid.to_be_bytes())
        .await
        .map_err(|_| "secured rfid info write failed")
}

/// Clears block 62 and restores the default key on sector 15.
#[cfg(not(feature = "e2e"))]
async fn unsign_card<I: Interface>(
    mfrc522: &mut Mfrc522<I>,
    card: &Uid,
) -> Result<(), &'static str> {
    mfrc522
        .authenticate(KeyType::A, 63, &Key(token_key()), card)
        .await
        .map_err(|_| "token key auth failed")?;
    _ = mfrc522.write_block(62, &[0; 16]).await;
    mfrc522
        .write_block(63, &sector_trailer(&Key::DEFAULT.0))
        .await
        .map_err(|_| "cannot unsecure card")
}

/// Whether block 62 (readable with the token key) holds this card's uid.
#[cfg(not(feature = "e2e"))]
async fn is_card_secured<I: Interface>(
    mfrc522: &mut Mfrc522<I>,
    card: &Uid,
    card_uid: u128,
) -> bool {
    if mfrc522
        .authenticate(KeyType::A, 63, &Key(token_key()), card)
        .await
        .is_err()
    {
        log::error!("Cannot auth card!");
        return false;
    }

    match mfrc522.read_block(62).await {
        Ok(buff) if u128::from_be_bytes(buff) == card_uid => true,
        Ok(buff) => {
            log::error!("Card is not secure!");
            log::debug!("read data: {buff:?}");
            false
        }
        Err(_) => {
            log::error!("Cannot read secured rfid info!");
            false
        }
    }
}

async fn handle_card<I: Interface>(
    mfrc522: &mut Mfrc522<I>,
    #[cfg(not(feature = "e2e"))] card: &Uid,
    card_uid: u128,
    global_state: &GlobalState,
) {
    #[cfg(not(feature = "e2e"))]
    {
        if !crate::state::trust_server() {
            log::error!("Skipping card scan. Server not trusted!");
            global_state.state.lock().await.error_text = Some("Server NOT Trusted!".into());
            _ = mfrc522.halt().await;
            return;
        }

        let menu_scene = global_state.state.value().await.menu_scene.clone();
        let sign_res = match menu_scene {
            Some(MenuScene::Signing) => Some(sign_card(mfrc522, card, card_uid).await),
            Some(MenuScene::Unsigning) => Some(unsign_card(mfrc522, card).await),
            _ => None,
        };

        if let Some(res) = sign_res {
            if let Err(e) = res {
                log::error!("[RFID] sign/unsign: {e}");
            }

            global_state.sign_unsign_progress.signal(res.is_ok());
            global_state.state.signal();
            _ = mfrc522.halt().await;
            _ = mfrc522.stop_crypto1().await;
            return;
        }

        if crate::state::secure_rfid() && !is_card_secured(mfrc522, card, card_uid).await {
            _ = mfrc522.halt().await;
            _ = mfrc522.stop_crypto1().await;
            return;
        }
    }

    let is_competitor = {
        let state = global_state.state.value().await;
        state.current_competitor.is_none()
            || state.current_competitor.unwrap_or_default() == card_uid as u64
    };

    let resp = crate::ws::request::<CardInfoResponsePacket>(
        crate::structs::TimerPacketInner::CardInfoRequest {
            card_id: card_uid as u64,
            is_competitor,
            attendance_device: None,
            sign_key: crate::state::sign_key(),
        },
        Duration::from_millis(crate::consts::WS_REQUEST_TIMEOUT_MS),
    )
    .await;

    match resp {
        Ok(resp) => process_card_info_response(resp, global_state).await,
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
    {
        let _ = mfrc522;
        crate::ws::send_test_ack(global_state).await;
    }

    #[cfg(not(feature = "e2e"))]
    {
        _ = mfrc522.halt().await;
        if crate::state::secure_rfid() {
            _ = mfrc522.stop_crypto1().await;
        }
    }
}

async fn process_card_info_response(resp: CardInfoResponsePacket, global_state: &GlobalState) {
    let mut state = global_state.state.lock().await;
    if state.should_skip_other_actions() {
        return;
    }

    match state.scene {
        Scene::WaitingForCompetitor if state.current_competitor.is_none() && resp.can_compete => {
            state.competitor_display = Some(resp.display);
            state.current_competitor = Some(resp.card_id);

            let competitor_locale = crate::translations::get_locale_index(&resp.country_iso2);
            if competitor_locale != crate::translations::current_locale_index() {
                crate::translations::select_locale_idx(competitor_locale, global_state);
            }

            match resp.possible_groups.first() {
                Some(group) if resp.possible_groups.len() == 1 => {
                    crate::state::set_group_limit(group.limit);
                    state.solve_group = Some(group.clone());
                    state.scene = if state.solve_time.is_some() {
                        Scene::Finished
                    } else {
                        Scene::CompetitorInfo
                    };
                }
                Some(_) => {
                    state.possible_groups = resp.possible_groups;
                    state.scene = Scene::GroupSelect;
                }
                None => {
                    state.reset_solve_state(None).await;
                    state.error_text = Some(
                        get_translation(TranslationKey::EMPTY_GROUPS_ERROR)
                            .as_str()
                            .into(),
                    );
                }
            }
        }
        Scene::Finished => {
            if state.current_competitor != Some(resp.card_id) && state.time_confirmed {
                state.current_judge = Some(resp.card_id);
            } else if let Some(current_competitor) = state.current_competitor
                && let Some(current_judge) = state.current_judge
                && current_competitor == resp.card_id
                && state.time_confirmed
            {
                let Some(solve_group) = state.solve_group.as_ref().map(|g| g.group_id.clone())
                else {
                    log::error!("Solve group is none! (How would that happen?)");
                    crate::log_once!(crate::utils::error_log::codes::RFID_SOLVE_GROUP_MISSING);
                    return;
                };
                let Some(solve_time) = state.solve_time else {
                    log::error!("Solve time is none!");
                    return;
                };

                #[cfg(not(feature = "e2e"))]
                if !crate::state::trust_server() {
                    log::error!("Skipping solve send. Server not trusted!");
                    return;
                }

                let inspection_time = match (state.inspection_start, state.inspection_end) {
                    (Some(start), Some(end)) if state.use_inspection() => {
                        end.saturating_duration_since(start).as_millis() as i64
                    }
                    _ => 0,
                };
                let session_id = state
                    .session_id
                    .get_or_insert_with(crate::state::new_session_id)
                    .clone();

                let packet = crate::structs::TimerPacketInner::Solve {
                    solve_time,
                    penalty: state.penalty.unwrap_or(0) as i64,
                    competitor_id: current_competitor,
                    judge_id: current_judge,
                    timestamp: current_epoch(),
                    session_id,
                    delegate: false,
                    inspection_time,
                    group_id: solve_group,
                    sign_key: crate::state::sign_key(),
                };

                // never hold the state lock across the server round-trip
                drop(state);
                let resp = crate::ws::request::<SolveConfirmPacket>(
                    packet,
                    Duration::from_millis(crate::consts::WS_REQUEST_TIMEOUT_MS),
                )
                .await;

                let Ok(resp) = resp else {
                    return;
                };
                log::warn!("solve_resp: {resp:?}");

                {
                    let mut state = global_state.state.lock().await;
                    // the solve may have been reset meanwhile (e.g. competitor reset hold)
                    if state.current_competitor == Some(current_competitor) {
                        state.reset_solve_state(Some(global_state.nvs)).await;
                    }

                    let (first, second) =
                        split_after_two_words(&resp.message).unwrap_or(("Solve", "sent"));
                    state.custom_message = Some((first.into(), second.into()));
                }

                Timer::after_millis(3000).await;
                global_state.state.lock().await.custom_message = None;
            } else if state.current_competitor == Some(resp.card_id)
                && state.current_judge.is_none()
                && state.time_confirmed
            {
                state.custom_message = Some((
                    get_translation(TranslationKey::CARDS_CANNOT_BE_THE_SAME_HEADER),
                    get_translation(TranslationKey::CARDS_CANNOT_BE_THE_SAME_FOOTER),
                ));
                drop(state);
                Timer::after_millis(8000).await;
                global_state.state.lock().await.custom_message = None;
            }
        }
        _ => {}
    }
}

/// `"a b c d"` -> `("a b", "c d")`; `None` for a single word.
fn split_after_two_words(msg: &str) -> Option<(&str, &str)> {
    let first = msg.find(' ')?;
    Some(match msg[first + 1..].find(' ') {
        Some(second) => (&msg[..first + 1 + second], &msg[first + 2 + second..]),
        None => (msg, ""),
    })
}

#[cfg(feature = "v4")]
async fn beep_card_scan(
    buzzer: &mut esp_hal::ledc::channel::Channel<'static, esp_hal::ledc::LowSpeed>,
    global_state: &GlobalState,
    sound_test: bool,
) {
    if global_state.state.value().await.sound_enabled || sound_test {
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
