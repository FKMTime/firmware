use alloc::rc::Rc;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
use esp_hal::{Async, i2c::master::I2c};

#[derive(Clone)]
pub struct SharedI2C {
    inner: Option<Rc<Mutex<NoopRawMutex, I2c<'static, Async>>>>,
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
        let Some(ref i2c) = self.inner else {
            return Err(esp_hal::i2c::master::Error::Timeout);
        };

        let mut guard = i2c.lock().await;
        let fut = embedded_hal_async::i2c::I2c::transaction(&mut *guard, address, operations);

        match embassy_time::with_timeout(embassy_time::Duration::from_millis(1500), fut).await {
            Ok(res) => res,
            Err(_) => {
                crate::utils::error_log::add_error(
                    crate::utils::error_log::codes::SHARED_I2C_TIMEOUT,
                )
                .await;
                Err(esp_hal::i2c::master::Error::Timeout)
            }
        }
    }
}

impl SharedI2C {
    pub fn new(i2c: Option<I2c<'static, Async>>) -> Self {
        match i2c {
            Some(i2c) => SharedI2C {
                inner: Some(Rc::new(Mutex::new(i2c))),
            },
            None => SharedI2C { inner: None },
        }
    }
}
