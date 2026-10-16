use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
use esp_hal::{Async, i2c::master::I2c};

type Bus = Mutex<NoopRawMutex, I2c<'static, Async>>;

#[derive(Clone, Copy)]
pub struct SharedI2C {
    inner: Option<&'static Bus>,
}

impl embedded_hal::i2c::ErrorType for SharedI2C {
    type Error = esp_hal::i2c::master::Error;
}

impl embedded_hal_async::i2c::I2c for SharedI2C {
    async fn transaction(
        &mut self,
        address: u8,
        operations: &mut [embedded_hal::i2c::Operation<'_>],
    ) -> Result<(), Self::Error> {
        let Some(i2c) = self.inner else {
            #[cfg(feature = "sim")]
            return crate::sim::i2c(address, operations);
            #[cfg(not(feature = "sim"))]
            return Err(esp_hal::i2c::master::Error::Timeout);
        };

        let mut guard = i2c.lock().await;
        let fut = embedded_hal_async::i2c::I2c::transaction(&mut *guard, address, operations);

        match embassy_time::with_timeout(embassy_time::Duration::from_millis(1500), fut).await {
            Ok(res) => res,
            Err(_) => {
                crate::utils::error_log::add_error(
                    crate::utils::error_log::codes::SHARED_I2C_TIMEOUT,
                );
                Err(esp_hal::i2c::master::Error::Timeout)
            }
        }
    }
}

impl SharedI2C {
    pub fn new(i2c: Option<I2c<'static, Async>>) -> Self {
        static BUS: static_cell::StaticCell<Bus> = static_cell::StaticCell::new();
        SharedI2C {
            inner: i2c.and_then(|i2c| BUS.try_init(Mutex::new(i2c)).map(|bus| &*bus)),
        }
    }
}
