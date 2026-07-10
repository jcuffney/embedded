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

// A `#![no_std]` LIBRARY is still testable with plain `cargo test`: the test
// binary runs on the host (your Mac, or the CI runner) and links `std`; only
// the library itself avoids it. This module is the repo's pattern for unit
// tests — inline `#[cfg(test)]`, next to the code it tests.
#[cfg(test)]
mod tests {
    use core::cell::Cell;
    use core::convert::Infallible;
    use core::future::Future;
    use core::pin::pin;
    use core::task::{Context, Waker};

    use embassy_time::{Duration, MockDriver};
    use embedded_hal::digital::{ErrorType, OutputPin, StatefulOutputPin};

    use super::blink;

    /// A test double for a GPIO pin: the software equivalent of wiring an
    /// LED to a breadboard and watching it. No mocking crate needed —
    /// implementing the two embedded-hal traits by hand is ~30 lines and
    /// shows exactly what a HAL provides.
    ///
    /// Why `&Cell` instead of plain fields: `blink(&mut pin, ..)` borrows
    /// the pin mutably for the whole life of its future, so the test could
    /// not read `pin.level` afterwards. The pin instead writes its state
    /// through shared `Cell`s that the test still owns. (`Cell` is `core`'s
    /// zero-cost single-threaded interior mutability — no std, no locks.)
    struct FakePin<'a> {
        level: &'a Cell<bool>,
        writes: &'a Cell<u32>,
    }

    impl ErrorType for FakePin<'_> {
        // A fake pin can never fail, and `Infallible` says so in the type
        // system (real HALs like esp-hal use it for GPIO too).
        type Error = Infallible;
    }

    impl OutputPin for FakePin<'_> {
        fn set_low(&mut self) -> Result<(), Infallible> {
            self.level.set(false);
            self.writes.set(self.writes.get() + 1);
            Ok(())
        }

        fn set_high(&mut self) -> Result<(), Infallible> {
            self.level.set(true);
            self.writes.set(self.writes.get() + 1);
            Ok(())
        }
    }

    impl StatefulOutputPin for FakePin<'_> {
        // `toggle()` — the method blink() actually calls — is a default
        // trait method built on top of these two plus set_low/set_high.
        fn is_set_high(&mut self) -> Result<bool, Infallible> {
            Ok(self.level.get())
        }

        fn is_set_low(&mut self) -> Result<bool, Infallible> {
            Ok(!self.level.get())
        }
    }

    /// `blink` never returns (`-> !`), so no executor could ever "finish"
    /// it. Instead we do what an executor does, by hand: poll the future
    /// once, let it park on its timer, advance a fake clock, poll again.
    /// This is the repo's pattern for testing async code deterministically —
    /// the test completes in microseconds regardless of the interval.
    ///
    /// NOTE: `MockDriver` is one process-global clock and `cargo test` runs
    /// tests on parallel threads — keep all time-advancing assertions inside
    /// this single test fn (or serialize tests) to avoid cross-talk.
    #[test]
    fn blink_toggles_once_per_interval() {
        let level = Cell::new(false);
        let writes = Cell::new(0);
        let mut pin = FakePin {
            level: &level,
            writes: &writes,
        };

        let interval = Duration::from_millis(100);
        // `pin!` (unrelated to GPIO pins!) stack-pins the future, which is
        // required before it can be polled. `Waker::noop()` is a waker that
        // ignores wake-ups — fine here because WE decide when to re-poll.
        let mut fut = pin!(blink(&mut pin, interval));
        let mut cx = Context::from_waker(Waker::noop());

        // First poll: blink toggles immediately, then parks on the timer.
        assert!(fut.as_mut().poll(&mut cx).is_pending());
        assert!(level.get(), "first toggle should drive the pin high");
        assert_eq!(writes.get(), 1);

        // Re-polling without advancing time must NOT toggle again — the
        // timer hasn't expired, so the future just goes back to sleep.
        assert!(fut.as_mut().poll(&mut cx).is_pending());
        assert_eq!(writes.get(), 1, "no time passed, so no extra toggle");

        // Advance the fake clock past the deadline; the next poll resumes
        // the loop body: one more toggle, then park on a fresh timer.
        MockDriver::get().advance(interval);
        assert!(fut.as_mut().poll(&mut cx).is_pending());
        assert!(!level.get(), "second toggle should drive the pin low");
        assert_eq!(writes.get(), 2);
    }
}
