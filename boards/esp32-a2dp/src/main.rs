//! Board bring-up: ESP32 as an A2DP source streaming to a Bluetooth speaker.
//!
//! Thin conductor, same philosophy as boards/esp32: all reusable logic lives
//! in capability crates (the melody comes from crates/audio), and this crate
//! only wires it to the hardware — here, the Bluetooth stack in src/bt.rs.

mod bt;

use std::sync::{Arc, Mutex};

use audio::{Melody, Note};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::log::EspLogger;

/// The demo tune: an ascending C-major arpeggio and a beat of silence, on
/// loop. `const` puts the table in flash; the `Melody` borrows it for the
/// `'static` lifetime the Bluetooth callbacks require.
const NOTES: [Note; 5] = [
    Note {
        freq_hz: 523,
        ms: 300,
    }, // C5
    Note {
        freq_hz: 659,
        ms: 300,
    }, // E5
    Note {
        freq_hz: 784,
        ms: 300,
    }, // G5
    Note {
        freq_hz: 1047,
        ms: 450,
    }, // C6
    Note {
        freq_hz: 0,
        ms: 650,
    }, // rest
];

/// ~25% of i16 full scale — plenty loud; the speaker's own volume does the rest.
const AMPLITUDE: i16 = 8_000;

fn main() -> anyhow::Result<()> {
    // Applies runtime patches keeping Rust std and the linked ESP-IDF
    // binary compatible. Every esp-idf binary must call this first.
    esp_idf_svc::sys::link_patches();
    // Route Rust's `log` macros into ESP-IDF's logger (the serial monitor).
    EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;

    // Compile-time injection from .env (see build.rs) — same pattern as the
    // WiFi credentials in boards/esp32.
    let speaker_name = env!(
        "SPEAKER_NAME",
        "Set SPEAKER_NAME in boards/esp32-a2dp/.env (copy .env.example)"
    );

    // The synthesizer is shared with the Bluetooth audio callback, which
    // runs on Bluedroid's task — hence Arc (shared ownership across threads)
    // + Mutex (one filler at a time). See bt.rs for the locking rules.
    let melody = Arc::new(Mutex::new(Melody::new(&NOTES, AMPLITUDE)));

    bt::run(peripherals.modem, speaker_name, melody)
}
