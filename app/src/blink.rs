//! Portable blink logic.
//!
//! This is a plain generic `async fn`, NOT an `#[embassy_executor::task]`.
//! Embassy tasks are statically allocated, so the macro can't handle generic
//! functions (it wouldn't know how many instances to reserve memory for).
//! The pattern: portable crates expose generic async fns, and each board
//! wraps one in a concrete task with its own pin type. See
//! boards/esp32/src/main.rs for the wrapper.

use embassy_time::{Duration, Timer};
use embedded_hal::digital::StatefulOutputPin;

/// Toggle `pin` forever, once per `interval`.
///
/// `StatefulOutputPin` is the embedded-hal trait for an output pin that can
/// read back its own state (which is what makes `toggle` possible). Any HAL's
/// GPIO output type implements it, which is what makes this chip-agnostic.
pub async fn blink(pin: &mut impl StatefulOutputPin, interval: Duration) -> ! {
    loop {
        // GPIO writes on a pin we own can't fail on real hardware; the
        // Result exists because the trait must also cover fallible expanders
        // (e.g. an I2C GPIO chip). Ignoring it here is deliberate.
        let _ = pin.toggle();
        Timer::after(interval).await;
    }
}
