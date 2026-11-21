//! v5 power: AXP2101 PMIC on the shared I²C bus (0x34). It makes VCC (DCDC1) and the MFRC522
//! supply (ALDO3), charges the 18650, measures it (fuel gauge + ADC) and owns the power key SW1.
//! Its IRQ line (open drain, active low) is on IO5. Pin-out and wiring: `pcbs/v5.md`.

use crate::{consts::BATTERY_SEND_INTERVAL_MS, utils::shared_i2c::SharedI2C};
use embassy_futures::select::select;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_hal_async::i2c::I2c;
use esp_hal::gpio::Input;
use portable_atomic::{AtomicBool, Ordering};

const ADDR: u8 = 0x34;
const CHIP_ID: u8 = 0x4A;

/// Register map (AXP2101 datasheet v1.4, §6.13).
mod reg {
    pub const STATUS1: u8 = 0x00;
    pub const CHIP_ID: u8 = 0x03;
    pub const INPUT_LIMIT: u8 = 0x16;
    pub const CHARGER_GAUGE: u8 = 0x18;
    pub const PWRON_STATUS: u8 = 0x20;
    pub const PWROFF_EN: u8 = 0x22;
    pub const VSYS_OFF: u8 = 0x24;
    pub const COMMON: u8 = 0x10;
    pub const SLEEP_WAKE: u8 = 0x26;
    pub const KEY_LEVELS: u8 = 0x27;
    pub const ADC_ENABLE: u8 = 0x30;
    pub const VBAT: u8 = 0x34;
    /// 0x40..=0x42
    pub const IRQ_ENABLE: u8 = 0x40;
    /// 0x48..=0x4A, write 1 to clear
    pub const IRQ_STATUS: u8 = 0x48;
    pub const TS_CTRL: u8 = 0x50;
    pub const ICC: u8 = 0x62;
    pub const CV: u8 = 0x64;
    pub const CHGLED: u8 = 0x69;
    pub const DCDC_EN: u8 = 0x80;
    pub const DCDC1_V: u8 = 0x82;
    pub const LDO_EN0: u8 = 0x90;
    pub const LDO_EN1: u8 = 0x91;
    pub const ALDO3_V: u8 = 0x94;
    pub const SOC: u8 = 0xA4;
}

/// IRQ status/enable bits, as one `u32` over the three registers (0x40/0x48 = bits 0..8).
mod irq {
    /// PWRON is active low: the positive edge is the release.
    pub const KEY_RELEASE: u32 = 1 << 8;
    pub const KEY_PRESS: u32 = 1 << (8 + 1);
    pub const BAT_REMOVE: u32 = 1 << (8 + 4);
    pub const BAT_INSERT: u32 = 1 << (8 + 5);
    pub const VBUS_REMOVE: u32 = 1 << (8 + 6);
    pub const VBUS_INSERT: u32 = 1 << (8 + 7);
    pub const CHARGE_START: u32 = 1 << (16 + 3);
    pub const CHARGE_DONE: u32 = 1 << (16 + 4);

    /// Only edge events: level ones (battery temperature, low SOC) stay set while the condition
    /// lasts and would hold the line low.
    pub const ENABLED: u32 = KEY_RELEASE
        | KEY_PRESS
        | BAT_REMOVE
        | BAT_INSERT
        | VBUS_REMOVE
        | VBUS_INSERT
        | CHARGE_START
        | CHARGE_DONE;
}

/// DCDC1 = 1.5 V + n × 0.1 V → 3.3 V (VCC: ESP32, OLED logic, keys, pull-ups).
const DCDC1_3V3: u8 = 18;
/// ALDO3 = 0.5 V + n × 0.1 V → 3.3 V (AVDD: every MFRC522 supply pin).
const ALDO3_3V3: u8 = 28;
const ALDO3_EN: u8 = 1 << 2;
/// Charge voltage 4.2 V. There is no protection IC on v5, so never leave this to the efuse.
const CV_4V2: u8 = 0b011;
/// Charge current: 200 mA + (n − 8) × 100 mA → 500 mA.
const ICC_500MA: u8 = 11;
/// The PMIC turns off below VSYS 2.6 V + n × 0.1 V → 3.0 V (no protection IC on the cell).
const VSYS_OFF_3V0: u8 = 4;
/// While off, the PMIC ignores SW1 until VSYS is somewhat above this threshold (about 3.24 V
/// at 3.0 V) and powers on by itself once it rises above it again. A power-off we chose puts
/// it back to 2.6 V so SW1 works at any cell voltage; after an undervoltage power-off it stays
/// at 3.0 V, so a sagging cell cannot restart the station in a loop.
const VSYS_OFF_2V6: u8 = 0;
/// REG 20H: the only power-on source was a cell being connected.
const PWRON_BATTERY_INSERT: u8 = 1 << 4;

#[derive(Debug, Clone, Copy)]
pub struct Reading {
    /// Fuel gauge state of charge, 0..=100.
    pub soc: u8,
    pub vbat_mv: u16,
    pub vbus_mv: u16,
    pub vsys_mv: u16,
    /// TS pin (board NTC R1, 50 µA source): about 500 mV at 25 °C, lower is warmer.
    pub ts_mv: u16,
    pub battery_present: bool,
    pub vbus_good: bool,
    /// `STATUS2[2:0]`: 0 tri-charge, 1 pre-charge, 2 CC, 3 CV, 4 done, 5 not charging.
    pub charge_state: u8,
}

#[derive(Clone, Copy)]
pub struct Axp2101 {
    i2c: SharedI2C,
}

type Error = esp_hal::i2c::master::Error;

impl Axp2101 {
    fn bus(&self) -> SharedI2C {
        self.i2c
    }

    async fn read(&self, reg: u8) -> Result<u8, Error> {
        let mut buf = [0];
        self.bus().write_read(ADDR, &[reg], &mut buf).await?;
        Ok(buf[0])
    }

    async fn read_n<const N: usize>(&self, reg: u8) -> Result<[u8; N], Error> {
        let mut buf = [0; N];
        self.bus().write_read(ADDR, &[reg], &mut buf).await?;
        Ok(buf)
    }

    async fn write(&self, reg: u8, value: u8) -> Result<(), Error> {
        self.bus().write(ADDR, &[reg, value]).await
    }

    /// Sets the bits of `mask` to `value`, keeps the others.
    async fn modify(&self, reg: u8, mask: u8, value: u8) -> Result<(), Error> {
        let old = self.read(reg).await?;
        let new = (old & !mask) | (value & mask);
        if new != old {
            self.write(reg, new).await?;
        }
        Ok(())
    }

    async fn irq_status(&self) -> Result<u32, Error> {
        let [a, b, c] = self.read_n::<3>(reg::IRQ_STATUS).await?;
        Ok(u32::from_le_bytes([a, b, c, 0]))
    }

    async fn clear_irqs(&self, bits: u32) -> Result<(), Error> {
        let [a, b, c, _] = bits.to_le_bytes();
        self.bus().write(ADDR, &[reg::IRQ_STATUS, a, b, c]).await
    }

    /// Three bus transactions (the bus is shared with the OLED and the RFID reader).
    pub async fn reading(&self) -> Result<Reading, Error> {
        let [status1, status2] = self.read_n::<2>(reg::STATUS1).await?;
        // VBAT, TS, VBUS, VSYS: 14 bits each, high byte first
        let adc = self.read_n::<8>(reg::VBAT).await?;
        let ch = |i: usize| (((adc[2 * i] & 0x3F) as u16) << 8) | adc[2 * i + 1] as u16;
        Ok(Reading {
            soc: self.read(reg::SOC).await?.min(100),
            vbat_mv: ch(0),
            ts_mv: ch(1) / 2,
            vbus_mv: ch(2),
            vsys_mv: ch(3),
            battery_present: status1 & (1 << 3) != 0,
            vbus_good: status1 & (1 << 5) != 0,
            charge_state: status2 & 0b111,
        })
    }

    /// Before ESP32 deep sleep: MFRC522 in soft power-down, nothing pending on IRQ (a low line
    /// wakes the ESP32, see `power::enter_deep_sleep`).
    ///
    /// ALDO3 stays on: an unpowered MFRC522 clamps SDA/SCL through its protection diodes, so
    /// after the wake nothing on the bus (this PMIC included) would answer to turn it back on.
    /// The driver's soft reset at boot ends the power-down.
    pub async fn prepare_deep_sleep(&self) -> Result<(), Error> {
        // CommandReg (0x01): PowerDown, command Idle
        if let Err(e) = self.bus().write(RFID_ADDR, &[0x01, 1 << 4]).await {
            log::error!("MFRC522 power-down failed: {e:?}");
        }
        self.clear_irqs(0xFF_FFFF).await
    }

    /// Puts the PMIC to sleep with ALDO3 (the MFRC522) off, until a power key press. The last
    /// bus access: the unpowered MFRC522 clamps SDA/SCL, so the PMIC restores ALDO3 by itself on
    /// wake (sleep mode, REG 26H). The press pulls IRQ low, which wakes the PMIC and the ESP32
    /// (`power::enter_soft_off`); the PMIC also pulses PWROK, resetting the ESP32 into a boot
    /// with a working bus. `init` restores the IRQs and leaves sleep mode.
    async fn sleep_until_key(&self) -> Result<(), Error> {
        // only a key press may pull IRQ low (not USB or charger events), and nothing pending
        let [a, b, c, _] = irq::KEY_PRESS.to_le_bytes();
        self.bus().write(ADDR, &[reg::IRQ_ENABLE, a, b, c]).await?;
        self.clear_irqs(0xFF_FFFF).await?;
        // IRQ-low wake, PWROK low on wake, wake to the voltages set now, sleep enable: the PMIC
        // records the rail enables at this point and restores them on wake
        self.write(reg::SLEEP_WAKE, 0b1_1101).await?;
        self.modify(reg::LDO_EN0, ALDO3_EN, 0).await
    }

    /// Real power-off (REG 10H bit 0): every rail off. SW1 turns it back on. On an unmodded v5
    /// the IRQ pull-up R4 goes to VCC, which dies here; the IRQ line then falls and can turn the
    /// PMIC straight back on (`pcbs/v5.md`), see [`SOFT_OFF`].
    async fn power_off(&self) -> Result<(), Error> {
        self.modify(reg::VSYS_OFF, 0b111, VSYS_OFF_2V6).await?;
        self.modify(reg::COMMON, 1 << 0, 1 << 0).await
    }

    /// The efuse powers the station on when a cell is connected (REG 20H bit 4), and no register
    /// turns that off. Without USB, power straight back off so only SW1 turns it on.
    async fn stay_off_after_battery_insert(&self) -> Result<(), Error> {
        let on = self.read(reg::PWRON_STATUS).await?;
        let vbus_good = self.read(reg::STATUS1).await? & (1 << 5) != 0;
        if on != PWRON_BATTERY_INSERT || vbus_good {
            return Ok(());
        }
        log::warn!("AXP2101: powered on by a battery insert, powering off");
        self.power_off().await?;
        loop {
            core::hint::spin_loop();
        }
    }

    async fn configure(&self) -> Result<(), Error> {
        // awake: no sleep mode, no IRQ-low wake (see `sleep_until_key`)
        self.modify(reg::SLEEP_WAKE, 0b1_0011, 0).await?;
        // rails: VCC 3.3 V and AVDD 3.3 V; the other outputs have nothing connected
        self.modify(reg::DCDC1_V, 0x1F, DCDC1_3V3).await?;
        self.modify(reg::DCDC_EN, 0b1_1111, 0b0_0001).await?;
        self.modify(reg::ALDO3_V, 0x1F, ALDO3_3V3).await?;
        self.write(reg::LDO_EN0, ALDO3_EN).await?;
        self.modify(reg::LDO_EN1, 1 << 0, 0).await?;

        // charger: 4.2 V / 500 mA, cell charging and the gauge on, power off at VSYS 3.0 V
        self.modify(reg::CV, 0b111, CV_4V2).await?;
        self.modify(reg::ICC, 0x1F, ICC_500MA).await?;
        self.modify(reg::CHARGER_GAUGE, 0b1010, 0b1010).await?;
        self.modify(reg::VSYS_OFF, 0b111, VSYS_OFF_3V0).await?;
        // CHGLED (LED2, only lit with USB in): charger-driven type A, on while charging,
        // 1 Hz blink on a charger fault
        self.modify(reg::CHGLED, 0b111, 0b001).await?;

        // ADC: battery, TS, VBUS, VSYS and die temperature
        self.modify(reg::ADC_ENABLE, 0b1_1111, 0b1_1111).await?;

        // SW1 (see `PowerKey`): a 128 ms press turns the station on (efuse: 512 ms); holding it
        // 6 s powers the PMIC off by itself (firmware hung), never restarting it
        self.modify(reg::KEY_LEVELS, 0b00_1111, 0b01 << 2).await?;
        self.modify(reg::PWROFF_EN, 0b11, 0b10).await?;

        let [a, b, c, _] = irq::ENABLED.to_le_bytes();
        self.bus().write(ADDR, &[reg::IRQ_ENABLE, a, b, c]).await?;
        self.clear_irqs(0xFF_FFFF).await
    }

    async fn log_registers(&self) -> Result<(), Error> {
        let [on, off] = self.read_n::<2>(reg::PWRON_STATUS).await?;
        log::info!("AXP2101: power-on source {on:#04x}, last power-off source {off:#04x}");
        for r in [
            reg::INPUT_LIMIT,
            reg::CHARGER_GAUGE,
            reg::PWROFF_EN,
            reg::VSYS_OFF,
            reg::KEY_LEVELS,
            reg::TS_CTRL,
            reg::ICC,
            reg::CV,
            reg::CHGLED,
            reg::DCDC_EN,
            reg::DCDC1_V,
            reg::LDO_EN0,
            reg::LDO_EN1,
            reg::ALDO3_V,
        ] {
            log::info!("AXP2101: reg {r:#04x} = {:#04x}", self.read(r).await?);
        }
        Ok(())
    }
}

static FOUND: AtomicBool = AtomicBool::new(false);

/// The PMIC on `i2c`, if [`init`] found it.
pub fn get(i2c: SharedI2C) -> Option<Axp2101> {
    FOUND.load(Ordering::Relaxed).then_some(Axp2101 { i2c })
}

/// Finds and configures the AXP2101. Must run before the RFID task starts (ALDO3 powers it).
pub async fn init(i2c: SharedI2C) -> Option<Axp2101> {
    let pmic = Axp2101 { i2c };
    let res = async {
        let id = pmic.read(reg::CHIP_ID).await?;
        if id != CHIP_ID {
            log::warn!("AXP2101: unexpected chip id {id:#04x} (expected {CHIP_ID:#04x})");
        }
        pmic.log_registers().await?;
        pmic.stay_off_after_battery_insert().await?;
        pmic.configure().await?;
        pmic.log_registers().await
    };
    match res.await {
        Ok(()) => {
            log::info!("AXP2101 configured");
            FOUND.store(true, Ordering::Relaxed);
            Some(pmic)
        }
        Err(e) => {
            log::error!("AXP2101 init failed: {e:?}");
            crate::utils::error_log::add_error(crate::utils::error_log::codes::PMIC_INIT_FAILED);
            None
        }
    }
}

const READ_INTERVAL: Duration = Duration::from_secs(1);
const MAX_RETRY_MS: u64 = 30_000;

/// Battery level, charging state and the PMIC's IRQs (power key, USB plug, charge events).
#[embassy_executor::task]
pub async fn pmic_task(
    i2c: SharedI2C,
    mut irq_pin: Input<'static>,
    state: crate::state::GlobalState,
) {
    let Some(pmic) = get(i2c) else {
        // no battery reading will come; don't hold the boot screen for one
        state.battery_ready.signal(());
        return;
    };

    let started = Instant::now();
    let mut sources_logged = false;
    let mut key = PowerKey::default();
    let mut last: Option<(u8, bool)> = None;
    let mut last_sent: Option<Instant> = None;
    let mut outage = false;
    let mut retry_ms = READ_INTERVAL.as_millis();
    loop {
        if let Ok(Ok(irqs)) = with_timeout(Duration::from_secs(3), pmic.irq_status()).await
            && irqs != 0
        {
            _ = pmic.clear_irqs(irqs).await;
            // disabled events set their status bits too
            let irqs = irqs & irq::ENABLED;
            if irqs != 0 {
                key.handle(irqs, started);
            }
        }
        if key.off_at().is_some_and(|at| Instant::now() >= at) {
            log::warn!("Power key held");
            turn_off(pmic).await;
        }

        let Ok(Ok(r)) = with_timeout(Duration::from_secs(3), pmic.reading()).await else {
            // a broken bus must not flood the error log or the shared i2c bus
            if !outage {
                outage = true;
                log::warn!("AXP2101 not responding");
                crate::utils::error_log::add_error(
                    crate::utils::error_log::codes::BATTERY_I2C_TIMEOUT,
                );
            }
            retry_ms = (retry_ms * 2).min(MAX_RETRY_MS);
            Timer::after_millis(retry_ms).await;
            continue;
        };
        if outage {
            outage = false;
            log::info!("AXP2101 responding again");
        }
        retry_ms = READ_INTERVAL.as_millis();

        // again once USB is up: the boot log is gone before the host opens the port
        if !sources_logged
            && started.elapsed() >= Duration::from_secs(3)
            && let Ok([on, off]) = pmic.read_n::<2>(reg::PWRON_STATUS).await
        {
            sources_logged = true;
            log::info!(
                "AXP2101: powered on by {on:#04x} (REG 20H), last off by {off:#04x} (REG 21H)"
            );
        }

        // the gauge needs a while after boot; until then use the voltage curve
        let percent = match r.soc {
            0 if r.battery_present => crate::utils::battery::percent(r.vbat_mv as f64),
            soc => soc,
        };
        let charging = r.vbus_good;
        if last != Some((percent, charging)) {
            state.battery.write(|b| {
                b.percent = percent;
                b.charging = charging;
            });
            state.battery_ready.signal(());
            crate::state::REDRAW.signal(());
            last = Some((percent, charging));
        }

        if last_sent.is_none_or(|t| t.elapsed().as_millis() >= BATTERY_SEND_INTERVAL_MS) {
            // without a cell the VBAT ADC reads garbage (~16 V)
            if r.battery_present
                && !crate::power::is_sleeping()
                && state.status.read(|s| s.server_connected) == Some(true)
            {
                _ = crate::ws::send(crate::structs::TimerPacket {
                    tag: None,
                    data: crate::structs::TimerPacketInner::Battery {
                        level: Some(percent as f64),
                        voltage: Some(r.vbat_mv as f64),
                    },
                });
            }

            log::info!(
                "Battery {}mV {}% (gauge {}%, present: {}, charge state {}), VBUS {}mV (good: {}), VSYS {}mV, TS {}mV",
                r.vbat_mv,
                percent,
                r.soc,
                r.battery_present,
                r.charge_state,
                r.vbus_mv,
                r.vbus_good,
                r.vsys_mv,
                r.ts_mv
            );
            last_sent = Some(Instant::now());
        }

        // edge-triggered, so an IRQ that stays asserted can't spin this loop; a held power key
        // wakes it when the hold is long enough
        let next = Instant::now() + READ_INTERVAL;
        let next = key.off_at().map_or(next, |at| at.min(next));
        select(irq_pin.wait_for_falling_edge(), Timer::at(next)).await;
    }
}

/// Station "off" with the PMIC still on (a real power-off can turn itself straight back on,
/// see `power::enter_soft_off`): display off, PMIC asleep with the RFID supply off, ESP32 in
/// deep sleep until SW1.
async fn soft_off(pmic: Axp2101) -> ! {
    log::warn!("Soft off (deep sleep until SW1)");
    // let the log line out before USB goes away
    Timer::after_millis(300).await;
    // SSD1309 display off: the panel's 12.5 V boost runs from VSYS whatever we do
    if let Err(e) = pmic.bus().write(OLED_ADDR, &[0x00, 0xAE]).await {
        log::error!("OLED off failed: {e:?}");
    }
    // last bus access, with no await after it: no other task can touch the bus in between
    if let Err(e) = pmic.sleep_until_key().await {
        log::error!("AXP2101 sleep failed: {e:?}");
    }
    crate::power::enter_soft_off()
}

/// What a [`POWER_OFF_HOLD`] does: `true` = soft off (PMIC asleep, ESP32 in deep sleep; safe on
/// an unmodded v5), `false` = real PMIC power-off (needs the IRQ pull-up on an always-on rail,
/// or the station can turn itself straight back on).
const SOFT_OFF: bool = false;

/// Turns the station off (power key, idle timeout): soft off or a real power-off, see
/// [`SOFT_OFF`]. Returns only if the PMIC could not be turned off.
pub async fn turn_off(pmic: Axp2101) {
    if SOFT_OFF {
        soft_off(pmic).await;
    }
    power_off(pmic).await;
}

/// Real power-off: every rail goes down, SW1 turns the station back on (a cold boot).
async fn power_off(pmic: Axp2101) {
    // dark at once: the finger can come off the key
    if let Err(e) = pmic.bus().write(OLED_ADDR, &[0x00, 0xAE]).await {
        log::error!("OLED off failed: {e:?}");
    }
    log::warn!("Powering off");
    // let the log line out before the rails go down
    Timer::after_millis(100).await;
    if let Err(e) = pmic.power_off().await {
        log::error!("AXP2101 power off failed: {e:?}");
    }
}

const OLED_ADDR: u8 = 0x3C;
const RFID_ADDR: u8 = 0x28;

/// SW1, the AXP2101's power key. Held for [`POWER_OFF_HOLD`], it turns the station off at once
/// (see [`SOFT_OFF`]), so the screen goes dark while the finger is still on it; a shorter press
/// wakes a sleeping display. When off, a press turns it on.
#[derive(Default)]
struct PowerKey {
    /// When the key went down, while it is down (and only after [`KEY_SETTLE`]).
    pressed_at: Option<Instant>,
}

const POWER_OFF_HOLD: Duration = Duration::from_millis(750);
/// Key events this soon after boot belong to the press that turned the station on, which must
/// not power it straight back off.
const KEY_SETTLE: Duration = Duration::from_millis(1500);

impl PowerKey {
    fn handle(&mut self, irqs: u32, started: Instant) {
        log::info!("AXP2101 IRQ {irqs:#08x}");
        if started.elapsed() < KEY_SETTLE {
            return;
        }
        if irqs & irq::KEY_RELEASE != 0 {
            // also a press and its release in one batch: a tap the task saw late
            self.pressed_at = None;
            if crate::power::is_sleeping() {
                // like any other key: wake the display (the press is consumed)
                crate::state::notify();
            }
        } else if irqs & irq::KEY_PRESS != 0 {
            self.pressed_at = Some(Instant::now());
        }
    }

    /// When the hold turns the station off, while the key is down.
    fn off_at(&self) -> Option<Instant> {
        self.pressed_at.map(|at| at + POWER_OFF_HOLD)
    }
}
