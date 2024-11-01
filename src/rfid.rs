use adv_shift_registers::wrappers::ShifterPin;
use embassy_time::{Duration, Timer};
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
) {
    let dma = Dma::new(dma);
    let dma_chan = dma.channel0;
    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(32000);
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
    _ = mfrc522.pcd_selftest().await;
    log::debug!("PCD ver: {:?}", mfrc522.pcd_get_version().await);

    if !mfrc522.pcd_is_init().await {
        log::error!("MFRC522 init failed! Try to power cycle to module!");
    }

    loop {
        if mfrc522.picc_is_new_card_present().await.is_ok() {
            let card = mfrc522.get_card(UidSize::Four).await;
            if let Ok(card) = card {
                log::info!("Card UID: {}", card.get_number());
            }

            _ = mfrc522.picc_halta().await;
        }

        Timer::after(Duration::from_millis(10)).await;
    }
}