use crate::{
    consts::{
        DEEPER_SLEEP_AFTER_MS, INSPECTION_TIME_PLUS2, LCD_INSPECTION_FRAME_TIME, SLEEP_AFTER_MS,
    },
    state::{
        ErrorLogEntryStage, GlobalState, MenuScene, Scene, SignaledGlobalStateInner,
        deeper_sleep_state, sleep_state,
    },
    translations::{TranslationKey, get_translation, get_translation_params},
    utils::{
        lcd_resourcese::{CrossedIcon, Resources},
        shared_i2c::SharedI2C,
        stackmat::ms_to_time_str,
    },
};
use alloc::{format, rc::Rc, string::String, string::ToString};
use anyhow::{Result, anyhow};
use display_interface_i2c::I2CInterface;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::{Delay, Instant, Timer};
use embedded_graphics::{
    Drawable,
    mono_font::MonoTextStyle,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
    text::{Alignment, Baseline, Text, TextStyle, TextStyleBuilder},
};
use embedded_graphics_framebuf::FrameBuf;
use embedded_layout::{
    layout::linear::{FixedMargin, Horizontal, LinearLayout},
    prelude::*,
    view_group::ViewGroup,
};
use embedded_text::{
    TextBox,
    alignment::{HorizontalAlignment, VerticalAlignment},
    style::TextBoxStyleBuilder,
};
use esp_hal::gpio::Output;
use oled_async::{displays::ssd1309::Ssd1309_128_64, mode::GraphicsMode};

pub const FBUF_WIDTH: usize = 128;
pub const FBUF_HEIGHT: usize = 64;
pub const FBUF_SIZE: usize = FBUF_WIDTH * FBUF_HEIGHT;

pub struct OledData<'a> {
    pub fbuf: FrameBuf<BinaryColor, &'a mut [BinaryColor; FBUF_SIZE]>,
    pub disp: GraphicsMode<Ssd1309_128_64, I2CInterface<SharedI2C>>,
}

impl OledData<'_> {
    pub async fn flush(&mut self) -> Result<()> {
        self.disp
            .draw_iter(&self.fbuf)
            .map_err(|e| anyhow!("{e:?}"))?;
        self.disp.flush().await.map_err(|e| anyhow!("{e:?}"))?;

        Ok(())
    }

    pub fn clear_main(&mut self) -> Result<()> {
        self.fbuf.fill_solid(&MAIN_RECT, BinaryColor::Off);
        Ok(())
    }
}

pub const MAIN_RECT: Rectangle = Rectangle::new(Point::new(0, 11), Size::new(128, 53));
pub const TOPBAR_RECT: Rectangle = Rectangle::new(Point::new(0, 0), Size::new(128, 10));
pub const DETAILS_VISIBLE_LINES: usize = 5;
pub const NORMAL_FONT: MonoTextStyle<'_, BinaryColor> = MonoTextStyle::new(
    &embedded_graphics::mono_font::ascii::FONT_7X13,
    BinaryColor::On,
);
pub const SMALL_FONT: MonoTextStyle<'_, BinaryColor> = MonoTextStyle::new(
    &embedded_graphics::mono_font::ascii::FONT_6X9,
    BinaryColor::On,
);
pub const TIMER_FONT: MonoTextStyle<'_, BinaryColor> =
    MonoTextStyle::new(&profont::PROFONT_14_POINT, BinaryColor::On);
pub const SMALL_TIMER_FONT: MonoTextStyle<'_, BinaryColor> =
    MonoTextStyle::new(&profont::PROFONT_7_POINT, BinaryColor::On);

pub const TEXT_CENTER: TextStyle = TextStyleBuilder::new()
    .alignment(Alignment::Center)
    .baseline(Baseline::Middle)
    .build();
pub const TEXT_TOPBAR: TextStyle = TextStyleBuilder::new()
    .alignment(Alignment::Right)
    .baseline(Baseline::Top)
    .build();

fn center_screen<VG: ViewGroup>(
    content: VG,
) -> LinearLayout<Horizontal<embedded_layout::align::vertical::Center>, VG> {
    LinearLayout::horizontal(content)
        .with_alignment(embedded_layout::align::vertical::Center)
        .arrange()
        .align_to(
            &Rectangle::new(Point::new(0, 0), Size::new(128, 64)),
            embedded_layout::align::horizontal::Center,
            embedded_layout::align::vertical::Center,
        )
}

fn center_layout<VG: ViewGroup>(
    content: VG,
) -> LinearLayout<Horizontal<embedded_layout::align::vertical::Center>, VG> {
    LinearLayout::horizontal(content)
        .with_alignment(embedded_layout::align::vertical::Center)
        .arrange()
        .align_to(
            &MAIN_RECT,
            embedded_layout::align::horizontal::Center,
            embedded_layout::align::vertical::Center,
        )
}

fn center_text_layout(text: &str) -> TextBox<'_, MonoTextStyle<'_, BinaryColor>> {
    let textbox_style = TextBoxStyleBuilder::new()
        .alignment(HorizontalAlignment::Center)
        .vertical_alignment(VerticalAlignment::Middle)
        .build();

    TextBox::with_textbox_style(text, MAIN_RECT, NORMAL_FONT, textbox_style)
}

fn draw_scrollable_menu<D>(target: &mut D, items: &[&str], selected: usize)
where
    D: DrawTarget<Color = BinaryColor>,
{
    const LINE_HEIGHT: i32 = 10;
    const VISIBLE: usize = 5;
    const PADDING_X: i32 = 4;

    let menu_font = MonoTextStyle::new(
        &embedded_graphics::mono_font::ascii::FONT_6X9,
        BinaryColor::On,
    );
    let menu_font_inv = MonoTextStyle::new(
        &embedded_graphics::mono_font::ascii::FONT_6X9,
        BinaryColor::Off,
    );

    let total = items.len();
    let scroll_start = if selected + 1 >= VISIBLE {
        (selected + 1 - VISIBLE).min(total.saturating_sub(VISIBLE))
    } else {
        0
    };

    let start_y = MAIN_RECT.top_left.y;

    for (row, item) in items[scroll_start..].iter().take(VISIBLE).enumerate() {
        let item_idx = scroll_start + row;
        let y = start_y + row as i32 * LINE_HEIGHT;
        let text_y = y + LINE_HEIGHT / 2;

        if item_idx == selected {
            let bar = Rectangle::new(Point::new(0, y), Size::new(128, LINE_HEIGHT as u32));
            bar.into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(target)
                .ok();
            Text::with_text_style(
                item,
                Point::new(PADDING_X, text_y),
                menu_font_inv,
                TextStyleBuilder::new()
                    .alignment(Alignment::Left)
                    .baseline(Baseline::Middle)
                    .build(),
            )
            .draw(target)
            .ok();
        } else {
            Text::with_text_style(
                item,
                Point::new(PADDING_X, text_y),
                menu_font,
                TextStyleBuilder::new()
                    .alignment(Alignment::Left)
                    .baseline(Baseline::Middle)
                    .build(),
            )
            .draw(target)
            .ok();
        }

        if scroll_start > 0 && row == 0 {
            Text::with_text_style("^", Point::new(122, text_y), menu_font, TEXT_CENTER)
                .draw(target)
                .ok();
        }
        if scroll_start + VISIBLE < total && row == VISIBLE - 1 {
            Text::with_text_style("v", Point::new(122, text_y), menu_font, TEXT_CENTER)
                .draw(target)
                .ok();
        }
    }
}

fn draw_scrollable_details_text<D>(target: &mut D, text: &str, scroll: usize)
where
    D: DrawTarget<Color = BinaryColor>,
{
    const LINE_HEIGHT: i32 = 10;
    const VISIBLE: usize = DETAILS_VISIBLE_LINES;
    const PADDING_X: i32 = 2;

    let font = MonoTextStyle::new(
        &embedded_graphics::mono_font::ascii::FONT_6X9,
        BinaryColor::On,
    );

    let lines: alloc::vec::Vec<&str> = text.split('\n').collect();
    let total = lines.len();
    let start_y = MAIN_RECT.top_left.y;

    for (row, line) in lines.iter().skip(scroll).take(VISIBLE).enumerate() {
        let y = start_y + row as i32 * LINE_HEIGHT;
        let text_y = y + LINE_HEIGHT / 2;
        Text::with_text_style(
            line,
            Point::new(PADDING_X, text_y),
            font,
            TextStyleBuilder::new()
                .alignment(Alignment::Left)
                .baseline(Baseline::Middle)
                .build(),
        )
        .draw(target)
        .ok();
    }

    if scroll > 0 {
        Text::with_text_style(
            "^",
            Point::new(122, start_y + LINE_HEIGHT / 2),
            font,
            TEXT_CENTER,
        )
        .draw(target)
        .ok();
    }
    if scroll + VISIBLE < total {
        let last_row_y = start_y + (VISIBLE as i32 - 1) * LINE_HEIGHT + LINE_HEIGHT / 2;
        Text::with_text_style("v", Point::new(122, last_row_y), font, TEXT_CENTER)
            .draw(target)
            .ok();
    }
}

#[allow(dead_code)]
pub fn draw_qr_code<D>(display: &mut D, text: &str, module_size: u32) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let max_version = match module_size {
        1 => qrcodegen_no_heap::Version::new(6),
        2 => qrcodegen_no_heap::Version::new(3),
        3 => qrcodegen_no_heap::Version::new(2),
        _ => qrcodegen_no_heap::Version::new(1),
    };

    let mut outbuffer = [0u8; qrcodegen_no_heap::Version::new(6).buffer_len()];
    let mut tempbuffer = [0u8; qrcodegen_no_heap::Version::new(6).buffer_len()];

    let qr = qrcodegen_no_heap::QrCode::encode_text(
        text,
        &mut tempbuffer,
        &mut outbuffer,
        qrcodegen_no_heap::QrCodeEcc::Quartile,
        qrcodegen_no_heap::Version::MIN,
        max_version,
        None,
        true,
    );

    if let Ok(qr) = qr {
        let filled = PrimitiveStyle::with_fill(BinaryColor::On);

        let qr_pixel_size = qr.size() as u32 * module_size;
        let origin = Point::new(
            ((128 - qr_pixel_size) / 2) as i32,
            ((64 - qr_pixel_size) / 2) as i32,
        );

        for y in 0..qr.size() {
            for x in 0..qr.size() {
                if qr.get_module(x, y) {
                    let top_left =
                        origin + Point::new(x * module_size as i32, y * module_size as i32);
                    Rectangle::new(top_left, Size::new(module_size, module_size))
                        .into_styled(filled)
                        .draw(display)?;
                }
            }
        }
    }

    Ok(())
}

#[embassy_executor::task]
pub async fn lcd_task(
    i2c: SharedI2C,
    mut display_rst: Output<'static>,
    global_state: GlobalState,
    wifi_setup_sig: Rc<Signal<NoopRawMutex, ()>>,
) {
    let di = display_interface_i2c::I2CInterface::new(i2c, 0x3C, 0x40);
    let raw_disp =
        oled_async::builder::Builder::new(oled_async::displays::ssd1309::Ssd1309_128_64 {})
            .with_rotation(oled_async::prelude::DisplayRotation::Rotate180)
            .connect(di);

    let mut disp: oled_async::mode::GraphicsMode<_, _> = raw_disp.into();
    disp.reset(&mut display_rst, &mut Delay);

    let disp_init = async {
        disp.init().await?;
        disp.clear();
        disp.flush().await?;

        anyhow::Result::<(), display_interface::DisplayError>::Ok(())
    }
    .await;

    if let Err(e) = disp_init {
        log::error!("Disp init error: {e:?} (but continuing i guess)");
    }

    let mut data = alloc::vec![embedded_graphics::pixelcolor::BinaryColor::Off; FBUF_SIZE];
    let Some(data): Option<&mut [BinaryColor; FBUF_SIZE]> = data.as_mut_array() else {
        log::error!("Disp framebuffer data alloc failed!");
        return;
    };
    let fbuf = embedded_graphics_framebuf::FrameBuf::new(data, FBUF_WIDTH, FBUF_HEIGHT);

    let mut oled = OledData { fbuf, disp };

    /*
    draw_qr_code(&mut oled.fbuf, "DSADSADSDSADSADSDSADSADSDSADSADSDSADSADSDSADSADS 42 DSADSADSDSADSADSDSADSADSDSADSADSDSADSADSDSADSADSDSADSADS", 1).unwrap();
    _ = oled.flush().await;

    Timer::after_millis(25000).await;
    */

    global_state.show_battery.wait().await;
    _ = process_top_bar(
        &global_state.state.value().await.clone(),
        &global_state,
        &mut oled,
    )
    .await;

    let text = format!(
        "S/N: {:X}\nVER: {}",
        crate::utils::get_efuse_u32(),
        crate::version::VERSION
    );
    let text = Text::with_text_style(&text, Point::zero(), NORMAL_FONT, TEXT_CENTER);

    center_layout(Chain::new(text)).draw(&mut oled.fbuf);
    _ = oled.flush().await;

    Timer::after_millis(2500).await;

    let mut last_update;
    loop {
        let current_state = global_state.state.value().await.clone();
        log::debug!("lcd current_state: {current_state:?}");
        last_update = Instant::now();

        if sleep_state() {
            unsafe {
                crate::state::SLEEP_STATE = false;
            }
            log::warn!("Sleep wakeup!");
        }

        let current_scene = current_state.scene.clone();
        let fut = async {
            oled.fbuf.clear(BinaryColor::Off);
            _ = process_top_bar(&current_state, &global_state, &mut oled).await;
            _ = process_main(&current_state, &global_state, &wifi_setup_sig, &mut oled).await;
            _ = oled.flush().await;

            loop {
                Timer::after_millis(1000).await;
                if global_state.show_battery.signaled() {
                    let state_snapshot = global_state.state.value().await.clone();
                    let qr_open = state_snapshot.menu_scene == Some(MenuScene::ErrorLog)
                        && state_snapshot.error_log_entry_stage == Some(ErrorLogEntryStage::Qr);
                    if qr_open {
                        continue;
                    }

                    oled.fbuf.fill_solid(&TOPBAR_RECT, BinaryColor::Off);
                    _ = process_top_bar(&state_snapshot, &global_state, &mut oled).await;
                    _ = oled.flush().await;

                    global_state.show_battery.reset();
                }

                #[cfg(not(any(feature = "e2e", feature = "qa")))]
                if !sleep_state()
                    && (Instant::now() - last_update).as_millis() > SLEEP_AFTER_MS
                    && current_scene.can_sleep()
                {
                    oled.fbuf.clear(BinaryColor::Off);
                    let text =
                        Text::with_text_style("Sleep", Point::zero(), SMALL_FONT, TEXT_CENTER);

                    center_screen(Chain::new(text)).draw(&mut oled.fbuf);
                    _ = oled.flush().await;

                    {
                        let mut state = global_state.state.lock().await;
                        state.server_connected = Some(false);
                        state.wifi_connected = Some(false);
                    }

                    unsafe {
                        crate::state::SLEEP_STATE = true;
                        crate::state::TRUST_SERVER = false;
                    }

                    global_state.state.signal_reset();
                }

                #[cfg(not(any(feature = "e2e", feature = "qa")))]
                if sleep_state()
                    && !deeper_sleep_state()
                    && (Instant::now() - last_update).as_millis() > DEEPER_SLEEP_AFTER_MS
                {
                    use esp_hal::rtc_cntl::{Rtc, sleep::RtcioWakeupSource};

                    oled.fbuf.clear(BinaryColor::Off);
                    let text =
                        Text::with_text_style("Sleep", Point::zero(), SMALL_FONT, TEXT_CENTER);

                    center_screen(Chain::new(text)).draw(&mut oled.fbuf);
                    _ = oled.flush().await;

                    unsafe {
                        use esp_hal::rtc_cntl::sleep::WakeupLevel;

                        let wakeup_pins: &mut [(
                            &mut dyn esp_hal::gpio::RtcPinWithResistors,
                            esp_hal::rtc_cntl::sleep::WakeupLevel,
                        )] = &mut [
                            (&mut esp_hal::peripherals::GPIO0::steal(), WakeupLevel::High),
                            (&mut esp_hal::peripherals::GPIO1::steal(), WakeupLevel::High),
                            (&mut esp_hal::peripherals::GPIO2::steal(), WakeupLevel::High),
                            (&mut esp_hal::peripherals::GPIO3::steal(), WakeupLevel::High),
                        ];
                        let rtcio = RtcioWakeupSource::new(wakeup_pins);
                        Rtc::new(esp_hal::peripherals::LPWR::steal()).sleep_deep(&[&rtcio]);
                    }
                }
            }
        };

        let res = embassy_futures::select::select(fut, global_state.state.wait()).await;
        match res {
            embassy_futures::select::Either::First(_) => {}
            embassy_futures::select::Either::Second(_) => {
                continue;
            }
        }
    }
}

fn battery_layout<VG: ViewGroup>(
    content: VG,
) -> LinearLayout<Horizontal<embedded_layout::align::vertical::Center, FixedMargin>, VG> {
    LinearLayout::horizontal(content)
        .with_alignment(embedded_layout::align::vertical::Center)
        .with_spacing(FixedMargin(1))
        .arrange()
        .align_to(
            &Rectangle::new(Point::new(0, 0), Size::new(128, 10)),
            embedded_layout::align::horizontal::Right,
            embedded_layout::align::vertical::Center,
        )
}

fn topbar_icons_layout<VG: ViewGroup>(
    content: VG,
) -> LinearLayout<Horizontal<embedded_layout::align::vertical::Center, FixedMargin>, VG> {
    LinearLayout::horizontal(content)
        .with_alignment(embedded_layout::align::vertical::Center)
        .with_spacing(FixedMargin(2))
        .arrange()
        .align_to(
            &Rectangle::new(Point::new(0, 0), Size::new(128, 10)),
            embedded_layout::align::horizontal::Left,
            embedded_layout::align::vertical::Center,
        )
}

async fn process_top_bar(
    current_state: &SignaledGlobalStateInner,
    _global_state: &GlobalState,
    oled: &mut OledData<'_>,
) -> Result<()> {
    let text = format!("{}%", current_state.battery_status.0);
    let text = Text::with_text_style(&text, Point::zero(), NORMAL_FONT, TEXT_TOPBAR);
    if current_state.battery_status.1 {
        battery_layout(Chain::new(Resources::CHARGING).append(text)).draw(&mut oled.fbuf)?;
    } else {
        battery_layout(Chain::new(text)).draw(&mut oled.fbuf)?;
    }

    topbar_icons_layout(
        Chain::new(CrossedIcon::new(
            Resources::WIFI,
            !current_state.wifi_connected.unwrap_or(false),
            9,
        ))
        .append(CrossedIcon::new(
            Resources::SERVER,
            !current_state.server_connected.unwrap_or(false),
            9,
        ))
        .append(CrossedIcon::new(
            Resources::TIMER,
            !current_state.stackmat_connected.unwrap_or(false),
            9,
        )),
    )
    .draw(&mut oled.fbuf)?;

    let text = if current_state.selected_config_menu.is_some() {
        Some("CONFIG")
    } else {
        match current_state.menu_scene {
            Some(MenuScene::Signing) => Some("SIGN"),
            Some(MenuScene::Unsigning) => Some("UNSIGN"),
            Some(MenuScene::BtDisplay) => Some("BTDISP"),
            Some(MenuScene::ErrorLog) => Some("ERRLOG"),
            Some(MenuScene::BuzzerVolume) => Some("BUZZER"),
            None => None,
        }
    };
    if let Some(text) = text {
        Text::with_text_style(text, Point::new(64, 5), SMALL_FONT, TEXT_CENTER)
            .draw(&mut oled.fbuf)?;
    }

    Line::new(Point::new(0, 10), Point::new(128, 10))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(&mut oled.fbuf)?;

    Ok(())
}

async fn process_main(
    current_state: &SignaledGlobalStateInner,
    global_state: &GlobalState,
    wifi_setup_sig: &Signal<NoopRawMutex, ()>,
    oled: &mut OledData<'_>,
) -> Result<()> {
    if let Some(ref error_text) = current_state.error_text {
        center_text_layout(&format!(
            "{}\n{}",
            get_translation(TranslationKey::ERROR_HEADER),
            error_text
        ))
        .draw(&mut oled.fbuf)?;

        return Ok(());
    }

    // display custom message on top of everything!
    if let Some((line1, line2)) = &current_state.custom_message {
        center_text_layout(&format!("{line1}\n{line2}")).draw(&mut oled.fbuf)?;
        return Ok(());
    }

    if let Some(sel) = current_state.selected_config_menu {
        let items: alloc::vec::Vec<alloc::string::String> = crate::structs::CONFIG_MENU_ITEMS
            .iter()
            .enumerate()
            .map(|(i, name)| alloc::format!("{}. {}", i + 1, name))
            .collect();
        let item_refs: alloc::vec::Vec<&str> = items.iter().map(|s| s.as_str()).collect();
        draw_scrollable_menu(&mut oled.fbuf, &item_refs, sel);
        return Ok(());
    }

    match current_state.menu_scene {
        Some(MenuScene::BuzzerVolume) => {
            center_text_layout(&format!(
                "Buzzer Volume\n< {} >\nSubmit to save",
                crate::state::buzzer_volume()
            ))
            .draw(&mut oled.fbuf)?;
            return Ok(());
        }
        Some(MenuScene::ErrorLog) => {
            if let Some(entry_idx) = current_state.selected_error_log_entry {
                if let Some(entry) = current_state.error_log_entries.get(entry_idx) {
                    if current_state.error_log_entry_stage == Some(ErrorLogEntryStage::Qr) {
                        let payload = format!("error-log-entry:{entry_idx}");
                        draw_qr_code(&mut oled.fbuf, &payload, 1)?;
                        return Ok(());
                    }

                    draw_scrollable_details_text(
                        &mut oled.fbuf,
                        &error_log_entry_details_text(entry),
                        current_state.error_log_details_scroll,
                    );
                    return Ok(());
                }

                center_text_layout("Invalid entry\nSubmit to return").draw(&mut oled.fbuf)?;
                return Ok(());
            }

            let mut items = current_state
                .error_log_entries
                .iter()
                .map(|entry| entry.list_label_v4())
                .collect::<alloc::vec::Vec<_>>();
            items.push("Exit".into());

            let sel = current_state.selected_error_log_item;
            if sel >= items.len() {
                global_state.state.lock().await.selected_error_log_item = 0;
            } else {
                let item_refs = items
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<alloc::vec::Vec<_>>();
                draw_scrollable_menu(&mut oled.fbuf, &item_refs, sel);
            }
            return Ok(());
        }
        Some(MenuScene::Signing) | Some(MenuScene::Unsigning) => {
            let prefix = if current_state.menu_scene == Some(MenuScene::Signing) {
                "S"
            } else {
                "Uns"
            };

            let main_text = format!("{prefix}igning");

            if global_state.sign_unsign_progress.signaled() {
                let status = if global_state.sign_unsign_progress.wait().await {
                    "OK"
                } else {
                    "FAIL"
                };

                center_text_layout(&format!("{main_text}\nOperation: {status}"))
                    .draw(&mut oled.fbuf)?;
                oled.flush().await?;

                Timer::after_millis(300).await;
                oled.clear_main()?;
                center_text_layout(&format!("{main_text}\nScan the card\n\nSubmit to exit"))
                    .draw(&mut oled.fbuf)?;
            } else {
                center_text_layout(&format!("{main_text}\nScan the card\n\nSubmit to exit"))
                    .draw(&mut oled.fbuf)?;
            }

            return Ok(());
        }
        Some(crate::state::MenuScene::BtDisplay) => {
            let mut items: alloc::vec::Vec<alloc::string::String> = current_state
                .discovered_bluetooth_devices
                .iter()
                .map(|dev| alloc::format!("{} [{:X?}]", dev.name, dev.addr))
                .collect();
            items.push("Unpair".into());
            items.push("Exit".into());

            let sel = current_state.selected_bluetooth_item;
            if sel >= items.len() {
                global_state.state.lock().await.selected_bluetooth_item = 0;
            } else {
                let item_refs: alloc::vec::Vec<&str> = items.iter().map(|s| s.as_str()).collect();
                draw_scrollable_menu(&mut oled.fbuf, &item_refs, sel);
            }
            return Ok(());
        }
        None => {}
    }

    let overwritten = process_main_overwrite(current_state, global_state, oled).await;
    if overwritten {
        return Ok(());
    }

    if let Some(time) = current_state.delegate_hold {
        let delegate_remaining = 3 - time;

        if delegate_remaining == 0 {
            center_text_layout(&format!(
                "{}\n{}",
                get_translation(TranslationKey::WAITING_FOR_DELEGATE_HEADER),
                get_translation(TranslationKey::WAITING_FOR_DELEGATE_FOOTER)
            ))
            .draw(&mut oled.fbuf)?;
        } else {
            center_text_layout(&format!(
                "{}\n{}",
                get_translation(TranslationKey::CALLING_FOR_DELEGATE_HEADER),
                get_translation_params(
                    TranslationKey::CALLING_FOR_DELEGATE_FOOTER,
                    &[delegate_remaining],
                )
            ))
            .draw(&mut oled.fbuf)?;
        }

        return Ok(());
    }

    match current_state.scene {
        Scene::WifiConnect => {
            center_text_layout(&format!(
                "{}\n{}",
                get_translation(TranslationKey::WAITING_FOR_WIFI_HEADER),
                get_translation(TranslationKey::WAITING_FOR_WIFI_FOOTER)
            ))
            .draw(&mut oled.fbuf)?;
            oled.flush().await?;

            wifi_setup_sig.wait().await;
            global_state.state.lock().await.scene = Scene::AutoSetupWait;
        }
        Scene::AutoSetupWait => {
            let wifi_ssid = alloc::format!("FKM-{:X}", crate::utils::get_efuse_u32());

            center_text_layout(&format!(
                "{}\n{wifi_ssid}",
                get_translation(TranslationKey::WIFI_SETUP_HEADER),
            ))
            .draw(&mut oled.fbuf)?;
        }
        Scene::MdnsWait => {
            center_text_layout(&format!(
                "{}\n{}",
                get_translation(TranslationKey::WAITING_FOR_MDNS_HEADER),
                get_translation(TranslationKey::WAITING_FOR_MDNS_FOOTER)
            ))
            .draw(&mut oled.fbuf)?;
        }
        Scene::GroupSelect => {
            let lt = Text::with_text_style("<", Point::zero(), TIMER_FONT, TEXT_CENTER);
            let gt = Text::with_text_style(">", Point::zero(), TIMER_FONT, TEXT_CENTER);
            LinearLayout::horizontal(Chain::new(lt))
                .with_alignment(embedded_layout::align::vertical::Center)
                .arrange()
                .align_to(
                    &MAIN_RECT,
                    embedded_layout::align::horizontal::Left,
                    embedded_layout::align::vertical::Center,
                )
                .draw(&mut oled.fbuf)?;

            LinearLayout::horizontal(Chain::new(gt))
                .with_alignment(embedded_layout::align::vertical::Center)
                .arrange()
                .align_to(
                    &MAIN_RECT,
                    embedded_layout::align::horizontal::Right,
                    embedded_layout::align::vertical::Center,
                )
                .draw(&mut oled.fbuf)?;

            center_text_layout(&format!(
                "{}\n{}",
                get_translation(TranslationKey::SELECT_GROUP),
                current_state.possible_groups[current_state.group_selected_idx].secondary_text
            ))
            .draw(&mut oled.fbuf)?;
        }
        Scene::WaitingForCompetitor => {
            if let Some(solve_time) = current_state.solve_time {
                let time_str = ms_to_time_str(solve_time);
                center_text_layout(&format!(
                    "{}\n{}",
                    get_translation(TranslationKey::SCAN_COMPETITOR_CARD_HEADER),
                    &get_translation_params(
                        TranslationKey::SCAN_COMPETITOR_CARD_WITH_TIME_FOOTER,
                        &[time_str],
                    )
                ))
                .draw(&mut oled.fbuf)?;
            } else {
                center_text_layout(&format!(
                    "{}\n{}",
                    get_translation(TranslationKey::SCAN_COMPETITOR_CARD_HEADER),
                    get_translation(TranslationKey::SCAN_COMPETITOR_CARD_FOOTER),
                ))
                .draw(&mut oled.fbuf)?;
            }
        }
        Scene::CompetitorInfo => {
            let mut text = current_state
                .competitor_display
                .clone()
                .unwrap_or("------".to_string())
                .to_string();

            if let Some(ref group) = current_state.solve_group {
                text += &format!("\n{}", group.secondary_text);
            }

            center_text_layout(&text).draw(&mut oled.fbuf)?;
        }
        Scene::Inspection => {
            oled.clear_main()?;
            oled.flush().await?;
            let inspection_start = global_state
                .state
                .value()
                .await
                .inspection_start
                .unwrap_or(Instant::now());

            _ = Text::with_text_style(
                &get_translation(TranslationKey::INSPECTION),
                Point::new(64, 50),
                NORMAL_FONT,
                TEXT_CENTER,
            )
            .draw(&mut oled.disp);

            let text_rect = Rectangle::new(Point::new(0, 28), Size::new(128, 17));
            loop {
                let elapsed = (Instant::now() - inspection_start).as_millis();
                let time_str = ms_to_time_str(elapsed);

                _ = oled.disp.fill_solid(&text_rect, BinaryColor::Off);
                _ = Text::with_text_style(&time_str, Point::new(64, 36), TIMER_FONT, TEXT_CENTER)
                    .draw(&mut oled.disp);
                _ = oled.disp.flush().await;

                Timer::after_millis(LCD_INSPECTION_FRAME_TIME).await;
            }
        }
        Scene::Timer => {
            oled.clear_main()?;
            oled.flush().await?;
            let text_rect = Rectangle::new(Point::new(0, 28), Size::new(128, 17));

            loop {
                let time = global_state.timer_signal.wait().await;
                let time_str = ms_to_time_str(time);

                _ = oled.disp.fill_solid(&text_rect, BinaryColor::Off);
                _ = Text::with_text_style(&time_str, Point::new(64, 36), TIMER_FONT, TEXT_CENTER)
                    .draw(&mut oled.disp);
                _ = oled.disp.flush().await;
            }
        }
        Scene::Finished => {
            let solve_time = current_state.solve_time.unwrap_or(0);
            let time_str = ms_to_time_str(solve_time);
            let inspection_time =
                match (current_state.inspection_start, current_state.inspection_end) {
                    (Some(start), Some(end)) => {
                        Some(end.saturating_duration_since(start).as_millis())
                    }
                    _ => None,
                };

            let show_inspection = current_state.use_inspection()
                && inspection_time.unwrap_or(0) > INSPECTION_TIME_PLUS2;
            let inspection_display =
                show_inspection.then(|| ms_to_time_str(inspection_time.unwrap_or(0)));

            let time_display = alloc::format!("{time_str}");
            let penalty = current_state.penalty.unwrap_or(0);
            let penalty_str: alloc::string::String = match penalty {
                -2 => "DNS".into(),
                -1 => "DNF".into(),
                1.. => alloc::format!("+{penalty}"),
                _ => alloc::string::String::new(),
            };

            let time_text =
                Text::with_text_style(&time_display, Point::zero(), TIMER_FONT, TEXT_CENTER);

            if let Some(insp_str) = inspection_display {
                let insp_str = format!(
                    "{}: {insp_str}",
                    get_translation(TranslationKey::INSPECTION)
                );
                let insp_text =
                    Text::with_text_style(&insp_str, Point::zero(), SMALL_TIMER_FONT, TEXT_CENTER);

                if penalty_str.is_empty() {
                    LinearLayout::vertical(
                        Chain::new(
                            LinearLayout::horizontal(Chain::new(time_text))
                                .with_alignment(embedded_layout::align::vertical::Center)
                                .arrange(),
                        )
                        .append(
                            LinearLayout::horizontal(Chain::new(insp_text))
                                .with_alignment(embedded_layout::align::vertical::Center)
                                .arrange(),
                        ),
                    )
                    .with_alignment(embedded_layout::align::horizontal::Center)
                    .arrange()
                    .align_to(
                        &MAIN_RECT,
                        embedded_layout::align::horizontal::Center,
                        embedded_layout::align::vertical::Top,
                    )
                    .translate(Point::new(0, 2))
                    .draw(&mut oled.fbuf)?;
                } else {
                    let penalty_text =
                        Text::with_text_style(&penalty_str, Point::zero(), TIMER_FONT, TEXT_CENTER);
                    LinearLayout::vertical(
                        Chain::new(
                            LinearLayout::horizontal(Chain::new(time_text).append(penalty_text))
                                .with_alignment(embedded_layout::align::vertical::Center)
                                .with_spacing(
                                    embedded_layout::layout::linear::spacing::FixedMargin(4),
                                )
                                .arrange(),
                        )
                        .append(
                            LinearLayout::horizontal(Chain::new(insp_text))
                                .with_alignment(embedded_layout::align::vertical::Center)
                                .arrange(),
                        ),
                    )
                    .with_alignment(embedded_layout::align::horizontal::Center)
                    .arrange()
                    .align_to(
                        &MAIN_RECT,
                        embedded_layout::align::horizontal::Center,
                        embedded_layout::align::vertical::Top,
                    )
                    .translate(Point::new(0, 2))
                    .draw(&mut oled.fbuf)?;
                }
            } else if penalty_str.is_empty() {
                LinearLayout::horizontal(Chain::new(time_text))
                    .with_alignment(embedded_layout::align::vertical::Center)
                    .arrange()
                    .align_to(
                        &MAIN_RECT,
                        embedded_layout::align::horizontal::Center,
                        embedded_layout::align::vertical::Top,
                    )
                    .translate(Point::new(0, 10))
                    .draw(&mut oled.fbuf)?;
            } else {
                let penalty_text =
                    Text::with_text_style(&penalty_str, Point::zero(), TIMER_FONT, TEXT_CENTER);
                LinearLayout::horizontal(Chain::new(time_text).append(penalty_text))
                    .with_alignment(embedded_layout::align::vertical::Center)
                    .with_spacing(embedded_layout::layout::linear::spacing::FixedMargin(4))
                    .arrange()
                    .align_to(
                        &MAIN_RECT,
                        embedded_layout::align::horizontal::Center,
                        embedded_layout::align::vertical::Top,
                    )
                    .translate(Point::new(0, 10))
                    .draw(&mut oled.fbuf)?;
            }

            let status = if !current_state.time_confirmed {
                Some(get_translation(TranslationKey::CONFIRM_TIME))
            } else if current_state.current_judge.is_none() {
                Some(get_translation(TranslationKey::SCAN_JUDGE_CARD))
            } else if current_state.current_competitor.is_some()
                && current_state.current_judge.is_some()
            {
                Some(get_translation(TranslationKey::SCAN_COMPETITOR_CARD))
            } else {
                None
            };
            if let Some(status_str) = status {
                let textbox_style = TextBoxStyleBuilder::new()
                    .alignment(HorizontalAlignment::Center)
                    .vertical_alignment(VerticalAlignment::Bottom)
                    .build();
                TextBox::with_textbox_style(&status_str, MAIN_RECT, NORMAL_FONT, textbox_style)
                    .draw(&mut oled.fbuf)?;
            }
        }
        Scene::Update => loop {
            let progress = global_state.update_progress.wait().await;
            oled.clear_main()?;
            center_text_layout(&format!("Updating\n{progress}%")).draw(&mut oled.fbuf)?;
            oled.flush().await?;
        },
    }

    Ok(())
}

pub fn error_log_entry_details_text(entry: &crate::utils::error_log::ErrorLogEntry) -> String {
    match entry {
        crate::utils::error_log::ErrorLogEntry::Code { timestamp, code } => format!(
            "Error E{code}\n{}\nSubmit to return",
            crate::utils::error_log::format_timestamp_full(*timestamp)
        ),
        crate::utils::error_log::ErrorLogEntry::Stacktrace {
            timestamp,
            version,
            addrs,
        } => {
            let mut details = format!(
                "Panic {}\nVER: {}\n",
                crate::utils::error_log::format_timestamp_compact(*timestamp),
                version
            );
            if addrs.is_empty() {
                details.push_str("No addrs");
            } else {
                for (idx, addr) in addrs.iter().enumerate() {
                    if idx > 0 {
                        details.push('\n');
                    }
                    details.push_str(&format!("0x{addr:X}"));
                }
            }
            details.push_str("\nSubmit to return");
            details
        }
    }
}

async fn process_main_overwrite(
    current_state: &SignaledGlobalStateInner,
    _global_state: &GlobalState,
    oled: &mut OledData<'_>,
) -> bool {
    if !current_state.scene.can_be_lcd_overwritten() {
        return false;
    }

    if current_state.server_connected == Some(false) {
        if current_state.wifi_connected == Some(false) {
            _ = center_text_layout("Wi-Fi\nConnection lost").draw(&mut oled.fbuf);
        } else {
            let text = format!(
                "{}\n{}",
                get_translation(TranslationKey::SERVER_DISCONNECTED_HEADER),
                get_translation(TranslationKey::SERVER_DISCONNECTED_FOOTER)
            );
            _ = center_text_layout(&text).draw(&mut oled.fbuf);
        }
    } else if current_state.device_added == Some(false) {
        #[cfg(not(feature = "e2e"))]
        let lines = (
            &get_translation(TranslationKey::DEVICE_NOT_ADDED_HEADER),
            &get_translation(TranslationKey::DEVICE_NOT_ADDED_FOOTER),
        );

        #[cfg(feature = "e2e")]
        let lines = ("Press submit", "To start HIL");

        let text = format!("{}\n{}", lines.0, lines.1);
        _ = center_text_layout(&text).draw(&mut oled.fbuf);
    } else if current_state.stackmat_connected == Some(false) {
        let text = format!(
            "{}\n{}",
            get_translation(TranslationKey::STACKMAT_DISCONNECTED_HEADER),
            get_translation(TranslationKey::STACKMAT_DISCONNECTED_FOOTER)
        );
        _ = center_text_layout(&text).draw(&mut oled.fbuf);
    } else {
        return false;
    }

    true
}
