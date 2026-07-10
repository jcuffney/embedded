//! ESP32 bring-up: everything in this file is chip-specific by design.
//! It initializes the hardware, starts the async runtime, then hands off to
//! portable logic in the `app` crate.

#![no_std]
#![no_main]

mod wifi;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    gpio::{Level, Output, OutputConfig},
    interrupt::software::SoftwareInterruptControl,
    rng::Rng,
    timer::timg::TimerGroup,
};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

// Template knobs: WiFi credentials come from `.env` (injected at compile time
// by build.rs — see .env.example), the URL is just a const to change.
const SSID: &str = env!("WIFI_SSID");
const PASSWORD: &str = env!("WIFI_PASSWORD");
const HTTP_URL: &str = "http://example.com/";

// DevKitC-32E has no user-controllable LED on board (the red one is power
// only), so wire an external LED to GPIO2 — the conventional "blinky" pin —
// or change this to whatever pin your board uses.
const BLINK_INTERVAL_MS: u64 = 500;

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // esp-radio (WiFi) allocates its internal buffers from this heap; 96 KiB
    // leaves comfortable headroom for it plus the app. See
    // docs/decisions/0003-buffers-heap-and-sockets.md.
    esp_alloc::heap_allocator!(size: 96 * 1024);
    esp_println::logger::init_logger_from_env();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    info!("blinky start");

    let led = Output::new(peripherals.GPIO2, Level::Low, OutputConfig::default());
    spawner.spawn(blink_task(led).expect("blink_task token"));

    let rng = Rng::new();
    let stack = wifi::start(spawner, rng, peripherals.WIFI, SSID, PASSWORD).await;

    match app::http::get_status(stack, HTTP_URL).await {
        Ok(code) => info!("HTTP status: {}", code),
        Err(e) => info!("HTTP request failed: {:?}", e),
    }

    // Bring-up is done; the spawned tasks (blink, wifi, net) do the ongoing
    // work. main must never return (`-> !`), so it parks here.
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

// The board-side wrapper for the portable blink logic: it pins down the
// concrete pin type (esp-hal's `Output`), which is the one thing the generic
// fn in `app` can't know. See app/src/blink.rs for why the split exists.
#[embassy_executor::task]
async fn blink_task(mut led: Output<'static>) -> ! {
    app::blink::blink(&mut led, Duration::from_millis(BLINK_INTERVAL_MS)).await
}
