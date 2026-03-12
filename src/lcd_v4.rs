use alloc::{format, rc::Rc, string::ToString};
use anyhow::{Result, anyhow};
use display_interface_i2c::I2CInterface;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::{Delay, Duration, Instant, Timer};
use embedded_graphics::{
    Drawable, Pixel,
    mono_font::MonoTextStyle,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Arc, Circle, Line, PrimitiveStyle, Rectangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use embedded_graphics_framebuf::FrameBuf;
use embedded_layout::{
    layout::linear::{FixedMargin, LinearLayout},
    prelude::*,
};
use esp_hal::gpio::Output;
use oled_async::{displays::ssd1309::Ssd1309_128_64, mode::GraphicsMode};
use profont::PROFONT_12_POINT;

use crate::{
    consts::{
        DEEPER_SLEEP_AFTER_MS, INSPECTION_TIME_PLUS2, LCD_INSPECTION_FRAME_TIME,
        SCROLL_TICKER_INVERVAL_MS, SLEEP_AFTER_MS,
    },
    state::{
        GlobalState, MenuScene, Scene, SignaledGlobalStateInner, deeper_sleep_state, sleep_state,
    },
    translations::{TranslationKey, get_translation, get_translation_params},
    utils::{shared_i2c::SharedI2C, stackmat::ms_to_time_str},
};

macros::load_lcd_resources!("src/resources");

#[derive(Clone, Copy)]
pub struct PixelArt {
    data: &'static [u8], // packed bits, MSB first, row by row
    width: u32,
    height: u32,
    top_left: Point,
}

impl PixelArt {
    pub const fn new(data: &'static [u8], width: u32, height: u32) -> Self {
        Self {
            data,
            width,
            height,
            top_left: Point::zero(),
        }
    }

    fn bytes_per_row(&self) -> u32 {
        (self.width + 7) / 8
    }
}

impl Dimensions for PixelArt {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(self.top_left, Size::new(self.width, self.height))
    }
}

impl Transform for PixelArt {
    fn translate(&self, by: Point) -> Self {
        Self {
            top_left: self.top_left + by,
            ..*self
        }
    }
    fn translate_mut(&mut self, by: Point) -> &mut Self {
        self.top_left += by;
        self
    }
}

impl Drawable for PixelArt {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D: DrawTarget<Color = BinaryColor>>(&self, target: &mut D) -> Result<(), D::Error> {
        let origin = self.top_left;
        let bpr = self.bytes_per_row();
        let (w, h) = (self.width, self.height);
        let data = self.data;

        target.draw_iter((0..h).flat_map(move |y| {
            (0..w).map(move |x| {
                let byte = data[(y * bpr + x / 8) as usize];
                let on = byte & (0x80 >> (x % 8)) != 0;
                Pixel(
                    origin + Point::new(x as i32, y as i32),
                    if on {
                        BinaryColor::On
                    } else {
                        BinaryColor::Off
                    },
                )
            })
        }))
    }
}

pub struct Overlay<A, B> {
    base: A,
    overlay: B,
    top_left: Point,
}

impl<A: Dimensions, B: Dimensions + Transform> Overlay<A, B> {
    pub fn new(base: A, overlay: B) -> Self {
        let base_box = base.bounding_box();
        let overlay_box = overlay.bounding_box();

        // center overlay over base
        let offset = Point::new(
            (base_box.size.width as i32 - overlay_box.size.width as i32) / 2,
            (base_box.size.height as i32 - overlay_box.size.height as i32) / 2,
        );
        let overlay = overlay.translate(base_box.top_left + offset);

        Self {
            top_left: base_box.top_left,
            base,
            overlay,
        }
    }
}

impl<A: Dimensions, B> Dimensions for Overlay<A, B> {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(self.top_left, self.base.bounding_box().size)
    }
}

impl<A: Transform, B: Transform> Transform for Overlay<A, B> {
    fn translate(&self, by: Point) -> Self {
        Self {
            top_left: self.top_left + by,
            base: self.base.translate(by),
            overlay: self.overlay.translate(by),
        }
    }
    fn translate_mut(&mut self, by: Point) -> &mut Self {
        self.top_left += by;
        self.base.translate_mut(by);
        self.overlay.translate_mut(by);
        self
    }
}

impl<A, B> Drawable for Overlay<A, B>
where
    A: Drawable<Color = BinaryColor>,
    B: Drawable<Color = BinaryColor>,
{
    type Color = BinaryColor;
    type Output = ();

    fn draw<D: DrawTarget<Color = BinaryColor>>(&self, target: &mut D) -> Result<(), D::Error> {
        self.base.draw(target)?;
        self.overlay.draw(target)?;
        Ok(())
    }
}

pub struct CrossedIcon<A> {
    base: A,
    top_left: Point,
    crossed: bool,
    cross_size: u32,
}

impl<A: Dimensions> CrossedIcon<A> {
    pub fn new(base: A, crossed: bool, cross_size: u32) -> Self {
        let base_box = base.bounding_box();
        Self {
            base,
            top_left: base_box.top_left,
            crossed,
            cross_size,
        }
    }
}

impl<A: Dimensions> Dimensions for CrossedIcon<A> {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(self.top_left, self.base.bounding_box().size)
    }
}

impl<A: Transform> Transform for CrossedIcon<A> {
    fn translate(&self, by: Point) -> Self {
        Self {
            top_left: self.top_left + by,
            base: self.base.translate(by),
            crossed: self.crossed,
            cross_size: self.cross_size,
        }
    }
    fn translate_mut(&mut self, by: Point) -> &mut Self {
        self.top_left += by;
        self.base.translate_mut(by);
        self
    }
}

impl<A> Drawable for CrossedIcon<A>
where
    A: Drawable<Color = BinaryColor> + Dimensions,
{
    type Color = BinaryColor;
    type Output = ();
    fn draw<D: DrawTarget<Color = BinaryColor>>(&self, target: &mut D) -> Result<(), D::Error> {
        self.base.draw(target)?;
        if self.crossed {
            let thin_stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
            let bb = self.bounding_box();

            // Center the cross over the bounding box
            let offset = Point::new(
                (bb.size.width as i32 - self.cross_size as i32) / 2,
                (bb.size.height as i32 - self.cross_size as i32) / 2,
            );
            let tl = bb.top_left + offset;
            let s = self.cross_size as i32 - 1;

            Line::new(tl, tl + Point::new(s, s))
                .into_styled(thin_stroke)
                .draw(target)?;
            Line::new(tl + Point::new(0, s), tl + Point::new(s, 0))
                .into_styled(thin_stroke)
                .draw(target)?;
        }
        Ok(())
    }
}

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
            .draw_iter(self.fbuf.into_iter())
            .map_err(|e| anyhow!("{e:?}"))?;
        self.disp.flush().await.map_err(|e| anyhow!("{e:?}"))?;

        Ok(())
    }
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
            .with_rotation(oled_async::prelude::DisplayRotation::Rotate0)
            .connect(di);

    let mut disp: oled_async::mode::GraphicsMode<_, _> = raw_disp.into();
    disp.reset(&mut display_rst, &mut Delay);

    // TODO: remove all these unwraps!
    disp.init().await.unwrap();
    disp.clear();
    disp.flush().await.unwrap();

    let mut data = alloc::vec![embedded_graphics::pixelcolor::BinaryColor::Off; FBUF_SIZE];
    let data: &mut [BinaryColor; FBUF_SIZE] = data.as_mut_array().unwrap();
    let fbuf = embedded_graphics_framebuf::FrameBuf::new(data, FBUF_WIDTH, FBUF_HEIGHT);

    let mut oled = OledData { fbuf, disp };

    global_state.show_battery.wait().await;
    _ = process_top_bar(
        &global_state.state.value().await.clone(),
        &global_state,
        &mut oled,
    )
    .await;

    let style = MonoTextStyle::new(
        &embedded_graphics::mono_font::ascii::FONT_7X13,
        BinaryColor::On,
    );
    let text_style = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .baseline(Baseline::Middle)
        .build();

    let text = format!(
        "S/N: {:X}\nVER: {}",
        crate::utils::get_efuse_u32(),
        crate::version::VERSION
    );
    let text = Text::with_text_style(&text, Point::zero(), style, text_style);

    LinearLayout::horizontal(Chain::new(text))
        .with_alignment(embedded_layout::align::vertical::Center)
        .arrange()
        .align_to(
            &MAIN_RECT,
            embedded_layout::align::horizontal::Center,
            embedded_layout::align::vertical::Center,
        )
        .draw(&mut oled.fbuf);
    _ = oled.flush().await;

    Timer::after_millis(2500).await;

    let mut last_update;
    loop {
        let current_state = global_state.state.value().await.clone();
        log::debug!("lcd current_state: {current_state:?}");
        last_update = Instant::now();

        if sleep_state() {
            //lcd.backlight_on();

            unsafe {
                crate::state::SLEEP_STATE = false;
            }
        }

        let current_scene = current_state.scene.clone();
        let fut = async {
            oled.fbuf.clear(BinaryColor::Off);
            _ = process_top_bar(&current_state, &global_state, &mut oled).await;
            _ = process_main(&current_state, &global_state, &wifi_setup_sig, &mut oled).await;
            _ = oled.flush().await;

            loop {
                Timer::after_millis(1000).await;

                #[cfg(not(any(feature = "e2e", feature = "qa")))]
                if !sleep_state()
                    && (Instant::now() - last_update).as_millis() > SLEEP_AFTER_MS
                    && current_scene.can_sleep()
                {
                    oled.fbuf.clear(BinaryColor::Off);

                    let style = MonoTextStyle::new(
                        &embedded_graphics::mono_font::ascii::FONT_7X13,
                        BinaryColor::On,
                    );
                    let text_style = TextStyleBuilder::new()
                        .alignment(Alignment::Center)
                        .baseline(Baseline::Middle)
                        .build();

                    let text = Text::with_text_style(
                        "Sleep\nPress any key...",
                        Point::zero(),
                        style,
                        text_style,
                    );

                    LinearLayout::horizontal(Chain::new(text))
                        .with_alignment(embedded_layout::align::vertical::Center)
                        .arrange()
                        .align_to(
                            &Rectangle::new(Point::new(0, 0), Size::new(128, 64)),
                            embedded_layout::align::horizontal::Center,
                            embedded_layout::align::vertical::Center,
                        )
                        .draw(&mut oled.fbuf);

                    _ = oled.flush().await;

                    {
                        global_state.state.lock().await.server_connected = Some(false);
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
                    oled.fbuf.clear(BinaryColor::Off);
                    let style = MonoTextStyle::new(
                        &embedded_graphics::mono_font::ascii::FONT_7X13,
                        BinaryColor::On,
                    );
                    let text_style = TextStyleBuilder::new()
                        .alignment(Alignment::Center)
                        .baseline(Baseline::Middle)
                        .build();

                    let text = Text::with_text_style(
                        "Deep Sleep\nPress any key...",
                        Point::zero(),
                        style,
                        text_style,
                    );

                    LinearLayout::horizontal(Chain::new(text))
                        .with_alignment(embedded_layout::align::vertical::Center)
                        .arrange()
                        .align_to(
                            &Rectangle::new(Point::new(0, 0), Size::new(128, 64)),
                            embedded_layout::align::horizontal::Center,
                            embedded_layout::align::vertical::Center,
                        )
                        .draw(&mut oled.fbuf);

                    _ = oled.flush().await;

                    crate::utils::deeper_sleep();
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

async fn process_top_bar(
    current_state: &SignaledGlobalStateInner,
    _global_state: &GlobalState,
    oled: &mut OledData<'_>,
) -> Result<()> {
    let style = MonoTextStyle::new(
        &embedded_graphics::mono_font::ascii::FONT_7X13,
        BinaryColor::On,
    );
    let text_style = TextStyleBuilder::new()
        .alignment(Alignment::Right)
        .baseline(Baseline::Top)
        .build();

    let text = format!("{}%", current_state.battery_status.0);
    let text = Text::with_text_style(&text, Point::zero(), style, text_style);
    if current_state.battery_status.1 {
        LinearLayout::horizontal(Chain::new(Resources::CHARGING).append(text))
            .with_alignment(embedded_layout::align::vertical::Center)
            .with_spacing(FixedMargin(1))
            .arrange()
            .align_to(
                &Rectangle::new(Point::new(0, 0), Size::new(128, 10)),
                embedded_layout::align::horizontal::Right,
                embedded_layout::align::vertical::Center,
            )
            .draw(&mut oled.fbuf)?;
    } else {
        LinearLayout::horizontal(Chain::new(text))
            .with_alignment(embedded_layout::align::vertical::Center)
            .with_spacing(FixedMargin(1))
            .arrange()
            .align_to(
                &Rectangle::new(Point::new(0, 0), Size::new(128, 10)),
                embedded_layout::align::horizontal::Right,
                embedded_layout::align::vertical::Center,
            )
            .draw(&mut oled.fbuf)?;
    }

    LinearLayout::horizontal(
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
    .with_alignment(embedded_layout::align::vertical::Center)
    .with_spacing(FixedMargin(2))
    .arrange()
    .align_to(
        &Rectangle::new(Point::new(0, 0), Size::new(128, 10)),
        embedded_layout::align::horizontal::Left,
        embedded_layout::align::vertical::Center,
    )
    .draw(&mut oled.fbuf)?;

    Line::new(Point::new(0, 10), Point::new(128, 10))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(&mut oled.fbuf)?;

    Ok(())
}

pub const MAIN_RECT: Rectangle = Rectangle::new(Point::new(0, 11), Size::new(128, 53));

async fn process_main(
    current_state: &SignaledGlobalStateInner,
    global_state: &GlobalState,
    wifi_setup_sig: &Signal<NoopRawMutex, ()>,
    oled: &mut OledData<'_>,
) -> Result<()> {
    let overwritten = process_main_overwrite(&current_state, global_state, oled).await;
    if overwritten {
        return Ok(());
    }

    match current_state.scene {
        Scene::Timer => {
            oled.fbuf.fill_solid(&MAIN_RECT, BinaryColor::Off);
            let text_rect = Rectangle::new(Point::new(0, 28), Size::new(128, 17));

            loop {
                let time = global_state.timer_signal.wait().await;
                let time_str = ms_to_time_str(time);
                let style = MonoTextStyle::new(&profont::PROFONT_14_POINT, BinaryColor::On);
                let text_style = TextStyleBuilder::new()
                    .alignment(Alignment::Center)
                    .baseline(Baseline::Middle)
                    .build();

                _ = oled.disp.fill_solid(&text_rect, BinaryColor::Off);
                _ = Text::with_text_style(&time_str, Point::new(64, 36), style, text_style)
                    .draw(&mut oled.disp);
                _ = oled.disp.flush().await;
            }
        }
        _ => {}
    }

    Ok(())
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
            let style = MonoTextStyle::new(
                &embedded_graphics::mono_font::ascii::FONT_7X13,
                BinaryColor::On,
            );
            let text_style = TextStyleBuilder::new()
                .alignment(Alignment::Center)
                .baseline(Baseline::Middle)
                .build();

            let text =
                Text::with_text_style("Wi-Fi\nConnection lost", Point::zero(), style, text_style);

            LinearLayout::horizontal(Chain::new(text))
                .with_alignment(embedded_layout::align::vertical::Center)
                .arrange()
                .align_to(
                    &MAIN_RECT,
                    embedded_layout::align::horizontal::Center,
                    embedded_layout::align::vertical::Center,
                )
                .draw(&mut oled.fbuf);
        } else {
            let style = MonoTextStyle::new(
                &embedded_graphics::mono_font::ascii::FONT_7X13,
                BinaryColor::On,
            );
            let text_style = TextStyleBuilder::new()
                .alignment(Alignment::Center)
                .baseline(Baseline::Middle)
                .build();

            let text = format!(
                "{}\n{}",
                get_translation(TranslationKey::SERVER_DISCONNECTED_HEADER),
                get_translation(TranslationKey::SERVER_DISCONNECTED_FOOTER)
            );
            let text = Text::with_text_style(&text, Point::zero(), style, text_style);

            LinearLayout::horizontal(Chain::new(text))
                .with_alignment(embedded_layout::align::vertical::Center)
                .arrange()
                .align_to(
                    &MAIN_RECT,
                    embedded_layout::align::horizontal::Center,
                    embedded_layout::align::vertical::Center,
                )
                .draw(&mut oled.fbuf);
        }
    } else if current_state.device_added == Some(false) {
        #[cfg(not(feature = "e2e"))]
        let lines = (
            &get_translation(TranslationKey::DEVICE_NOT_ADDED_HEADER),
            &get_translation(TranslationKey::DEVICE_NOT_ADDED_FOOTER),
        );

        #[cfg(feature = "e2e")]
        let lines = ("Press submit", "To start HIL");

        let style = MonoTextStyle::new(
            &embedded_graphics::mono_font::ascii::FONT_7X13,
            BinaryColor::On,
        );
        let text_style = TextStyleBuilder::new()
            .alignment(Alignment::Center)
            .baseline(Baseline::Middle)
            .build();

        let text = format!("{}\n{}", lines.0, lines.1);
        let text = Text::with_text_style(&text, Point::zero(), style, text_style);

        LinearLayout::horizontal(Chain::new(text))
            .with_alignment(embedded_layout::align::vertical::Center)
            .arrange()
            .align_to(
                &MAIN_RECT,
                embedded_layout::align::horizontal::Center,
                embedded_layout::align::vertical::Center,
            )
            .draw(&mut oled.fbuf);
    } else if current_state.stackmat_connected == Some(false) {
        let style = MonoTextStyle::new(&profont::PROFONT_14_POINT, BinaryColor::On);
        let text_style = TextStyleBuilder::new()
            .alignment(Alignment::Center)
            .baseline(Baseline::Middle)
            .build();

        let text = format!(
            "{}\n{}",
            get_translation(TranslationKey::STACKMAT_DISCONNECTED_HEADER),
            get_translation(TranslationKey::STACKMAT_DISCONNECTED_FOOTER)
        );
        let text = Text::with_text_style(&text, Point::zero(), style, text_style);

        LinearLayout::horizontal(Chain::new(text))
            .with_alignment(embedded_layout::align::vertical::Center)
            .arrange()
            .align_to(
                &MAIN_RECT,
                embedded_layout::align::horizontal::Center,
                embedded_layout::align::vertical::Center,
            )
            .draw(&mut oled.fbuf);
    } else {
        return false;
    }

    true
}

/*
async fn process_lcd(
    current_state: SignaledGlobalStateInner,
    global_state: &GlobalState,
    lcd_driver: &mut LcdAbstract<80, 16, 2, 3>,
    //lcd: &mut LcdDisplay<T, D>,
    wifi_setup_sig: &Signal<NoopRawMutex, ()>,
    //display: &ShifterValueRange,
    fbuf: &mut FrameBuf<BinaryColor, &mut [BinaryColor; 128 * 64]>,
    disp: &mut GraphicsMode<Ssd1309_128_64, I2CInterface<SharedI2C>>,
) -> Option<()> {
    #[cfg(feature = "bat_dev_lcd")]
    {
        let battery_read = current_state.current_bat_read.unwrap_or(-1.0);
        lcd_driver
            .print(
                0,
                &alloc::format!("BAT: {battery_read}"),
                PrintAlign::Left,
                true,
            )
            .ok()?;

        if let Some(avg) = current_state.avg_bat_read {
            lcd_driver
                .print(1, &alloc::format!("AVG: {avg}"), PrintAlign::Left, true)
                .ok()?;
        }

        return Some(());
    }

    if let Some(error_text) = current_state.error_text {
        lcd_driver
            .print(
                0,
                &get_translation(TranslationKey::ERROR_HEADER),
                PrintAlign::Center,
                true,
            )
            .ok()?;

        lcd_driver
            .print(1, &error_text, PrintAlign::Center, true)
            .ok()?;

        return Some(());
    }

    // display custom message on top of everything!
    if let Some((line1, line2)) = &current_state.custom_message {
        _ = lcd_driver.print(0, line1, PrintAlign::Center, true);
        _ = lcd_driver.print(1, line2, PrintAlign::Center, true);

        return Some(());
    }

    if let Some(sel) = current_state.selected_config_menu {
        lcd_driver.clear_all().ok()?;
        lcd_driver.print(0, "<", PrintAlign::Left, false).ok()?;
        lcd_driver.print(0, ">", PrintAlign::Right, false).ok()?;

        lcd_driver
            .print(0, "Config Menu", PrintAlign::Center, false)
            .ok()?;

        lcd_driver
            .print(
                1,
                &alloc::format!("{}. {}", sel + 1, crate::structs::CONFIG_MENU_ITEMS[sel]),
                PrintAlign::Left,
                true,
            )
            .ok()?;

        return Some(());
    }

    match current_state.menu_scene {
        Some(MenuScene::Signing) | Some(MenuScene::Unsigning) => {
            let prefix = if current_state.menu_scene == Some(MenuScene::Signing) {
                "S"
            } else {
                "Uns"
            };

            lcd_driver.clear_all().ok()?;
            lcd_driver
                .print(
                    0,
                    &alloc::format!("{prefix}igning | Submit To Exit"),
                    PrintAlign::Left,
                    true,
                )
                .ok()?;

            if global_state.sign_unsign_progress.signaled() {
                let status = if global_state.sign_unsign_progress.wait().await {
                    "OK"
                } else {
                    "FAIL"
                };

                lcd_driver
                    .print(
                        1,
                        &alloc::format!("Operation {}", status),
                        PrintAlign::Center,
                        true,
                    )
                    .ok()?;
                fbuf.clear(BinaryColor::Off);
                lcd_driver.display_on_oled(fbuf).await;
                embedded_graphics::prelude::DrawTarget::draw_iter(disp, fbuf.into_iter()).unwrap();
                disp.flush().await.unwrap();

                Timer::after_millis(300).await;
                lcd_driver
                    .print(1, "Scan the card", PrintAlign::Center, true)
                    .ok()?;
            } else {
                lcd_driver
                    .print(1, "Scan the card", PrintAlign::Center, true)
                    .ok()?;
            }

            return Some(());
        }
        Some(crate::state::MenuScene::BtDisplay) => {
            lcd_driver.clear_all().ok()?;
            if current_state.selected_bluetooth_item
                == current_state.discovered_bluetooth_devices.len()
            {
                lcd_driver
                    .print(0, "Unpair", PrintAlign::Center, true)
                    .ok()?;
            } else if current_state.selected_bluetooth_item
                == current_state.discovered_bluetooth_devices.len() + 1
            {
                lcd_driver.print(0, "Exit", PrintAlign::Center, true).ok()?;
            } else if current_state.selected_bluetooth_item
                < current_state.discovered_bluetooth_devices.len()
            {
                if let Some(display_dev) = current_state
                    .discovered_bluetooth_devices
                    .get(current_state.selected_bluetooth_item)
                {
                    lcd_driver
                        .print(0, &display_dev.name, PrintAlign::Center, true)
                        .ok()?;

                    lcd_driver
                        .print(
                            1,
                            &alloc::format!("{:x?}", display_dev.addr),
                            PrintAlign::Center,
                            true,
                        )
                        .ok()?;
                }
            } else {
                global_state.state.lock().await.selected_bluetooth_item = 0;
            }

            return Some(());
        }
        None => {}
    }

    let overwritten = process_lcd_overwrite(&current_state, global_state, lcd_driver).await;
    if overwritten {
        return Some(());
    }

    lcd_driver.clear_all().ok()?;
    if let Some(time) = current_state.delegate_hold {
        let delegate_remaining = 3 - time;

        if delegate_remaining == 0 {
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::WAITING_FOR_DELEGATE_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &get_translation(TranslationKey::WAITING_FOR_DELEGATE_FOOTER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;
        } else {
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::CALLING_FOR_DELEGATE_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &get_translation_params(
                        TranslationKey::CALLING_FOR_DELEGATE_FOOTER,
                        &[delegate_remaining],
                    ),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;
        }

        return Some(());
    }

    match current_state.scene {
        Scene::WifiConnect => {
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::WAITING_FOR_WIFI_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &get_translation(TranslationKey::WAITING_FOR_WIFI_FOOTER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            fbuf.clear(BinaryColor::Off);
            lcd_driver.display_on_oled(fbuf).await;
            embedded_graphics::prelude::DrawTarget::draw_iter(disp, fbuf.into_iter()).unwrap();
            disp.flush().await.unwrap();
            wifi_setup_sig.wait().await;
            global_state.state.lock().await.scene = Scene::AutoSetupWait;
        }
        Scene::AutoSetupWait => {
            let wifi_ssid = alloc::format!("FKM-{:X}", crate::utils::get_efuse_u32());
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::WIFI_SETUP_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(1, &wifi_ssid, PrintAlign::Center, true)
                .ok()?;
        }
        Scene::MdnsWait => {
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::WAITING_FOR_MDNS_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &get_translation(TranslationKey::WAITING_FOR_MDNS_FOOTER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;
        }
        Scene::GroupSelect => {
            lcd_driver.clear_all().ok()?;
            lcd_driver.print(0, "<", PrintAlign::Left, false).ok()?;
            lcd_driver.print(1, "<", PrintAlign::Left, false).ok()?;
            lcd_driver.print(0, ">", PrintAlign::Right, false).ok()?;
            lcd_driver.print(1, ">", PrintAlign::Right, false).ok()?;

            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::SELECT_GROUP),
                    PrintAlign::Center,
                    false,
                )
                .ok()?;

            lcd_driver
                .print(
                    1,
                    &current_state.possible_groups[current_state.group_selected_idx].secondary_text,
                    PrintAlign::Center,
                    false,
                )
                .ok()?;
        }
        Scene::WaitingForCompetitor => {
            lcd_driver
                .print(
                    0,
                    &get_translation(TranslationKey::SCAN_COMPETITOR_CARD_HEADER),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            if let Some(solve_time) = current_state.solve_time {
                let time_str = ms_to_time_str(solve_time);
                lcd_driver
                    .print(
                        1,
                        &get_translation_params(
                            TranslationKey::SCAN_COMPETITOR_CARD_WITH_TIME_FOOTER,
                            &[time_str],
                        ),
                        PrintAlign::Center,
                        true,
                    )
                    .ok()?;
            } else {
                lcd_driver
                    .print(
                        1,
                        &get_translation(TranslationKey::SCAN_COMPETITOR_CARD_FOOTER),
                        PrintAlign::Center,
                        true,
                    )
                    .ok()?;
            }
        }
        Scene::CompetitorInfo => {
            lcd_driver
                .print(
                    0,
                    &current_state
                        .competitor_display
                        .unwrap_or("??????".to_string()),
                    PrintAlign::Center,
                    true,
                )
                .ok()?;

            if let Some(group) = current_state.solve_group {
                lcd_driver
                    .print(1, &group.secondary_text, PrintAlign::Center, true)
                    .ok()?;
            }
        }
        Scene::Inspection => {
            let inspection_start = global_state
                .state
                .value()
                .await
                .inspection_start
                .unwrap_or(Instant::now());

            lcd_driver
                .print(1, "Inspection", PrintAlign::Center, true)
                .ok()?;

            loop {
                let elapsed = (Instant::now() - inspection_start).as_millis();
                let time_str = ms_to_time_str(elapsed);

                lcd_driver
                    .print(0, &time_str, PrintAlign::Center, true)
                    .ok()?;

                fbuf.clear(BinaryColor::Off);
                lcd_driver.display_on_oled(fbuf).await;
                embedded_graphics::prelude::DrawTarget::draw_iter(disp, fbuf.into_iter()).unwrap();
                disp.flush().await.unwrap();
                Timer::after_millis(LCD_INSPECTION_FRAME_TIME).await;
            }
        }
        Scene::Timer => loop {
            let time = global_state.timer_signal.wait().await;
            let time_str = ms_to_time_str(time);
            lcd_driver
                .print(0, &time_str, PrintAlign::Center, true)
                .ok()?;

            //display.set_data_raw(&crate::utils::stackmat::time_str_to_display(&time_str));
            fbuf.clear(BinaryColor::Off);
            lcd_driver.display_on_oled(fbuf).await;
            embedded_graphics::prelude::DrawTarget::draw_iter(disp, fbuf.into_iter()).unwrap();
            disp.flush().await.unwrap();
        },
        Scene::Finished => {
            let solve_time = current_state.solve_time.unwrap_or(0);
            let time_str = if solve_time > 0 {
                ms_to_time_str(solve_time)
            } else {
                heapless::String::new()
            };

            let inspection_time =
                match (current_state.inspection_start, current_state.inspection_end) {
                    (Some(start), Some(end)) => {
                        Some(end.saturating_duration_since(start).as_millis())
                    }
                    _ => None,
                };

            if current_state.use_inspection()
                && inspection_time.unwrap_or(0) > INSPECTION_TIME_PLUS2
            {
                let inspections_seconds = inspection_time.unwrap_or(0) / 1000;
                lcd_driver
                    .print(
                        0,
                        &alloc::format!("{time_str} ({inspections_seconds}s)"),
                        PrintAlign::Left,
                        true,
                    )
                    .ok()?;
            } else {
                lcd_driver
                    .print(0, &time_str, PrintAlign::Left, true)
                    .ok()?;
            }

            let penalty = current_state.penalty.unwrap_or(0);
            let penalty_str = match penalty {
                -2 => "DNS",
                -1 => "DNF",
                1.. => &alloc::format!("+{penalty}"),
                _ => "",
            };

            lcd_driver
                .print(0, penalty_str, PrintAlign::Right, false)
                .ok()?;

            if !current_state.time_confirmed {
                lcd_driver
                    .print(
                        1,
                        &get_translation(TranslationKey::CONFIRM_TIME),
                        PrintAlign::Right,
                        true,
                    )
                    .ok()?;
            } else if current_state.current_judge.is_none() {
                lcd_driver
                    .print(
                        1,
                        &get_translation(TranslationKey::SCAN_JUDGE_CARD),
                        PrintAlign::Right,
                        true,
                    )
                    .ok()?;
            } else if current_state.current_competitor.is_some()
                && current_state.current_judge.is_some()
            {
                lcd_driver
                    .print(
                        1,
                        &get_translation(TranslationKey::SCAN_COMPETITOR_CARD),
                        PrintAlign::Right,
                        true,
                    )
                    .ok()?;
            }
        }
        Scene::Update => {
            _ = lcd_driver.print(0, "Updating...", PrintAlign::Center, true);
            loop {
                let progress = global_state.update_progress.wait().await;
                _ = lcd_driver.print(1, &alloc::format!("{progress}%"), PrintAlign::Center, true);

                fbuf.clear(BinaryColor::Off);
                lcd_driver.display_on_oled(fbuf).await;
                embedded_graphics::prelude::DrawTarget::draw_iter(disp, fbuf.into_iter()).unwrap();
                disp.flush().await.unwrap();
            }
        }
    }

    Some(())
}
*/
