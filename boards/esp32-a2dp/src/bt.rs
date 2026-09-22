//! Bluetooth Classic bring-up: discover a speaker by name, pair, and stream
//! audio to it as an A2DP source.
//!
//! ## The threading model (read this first)
//!
//! Everything Bluetooth here runs on **Bluedroid's own FreeRTOS task**, not
//! ours: the closures we pass to `subscribe()` are invoked from inside the C
//! stack. Two rules fall out:
//!
//! 1. **Callbacks stay dumb.** Each control event is translated into a small
//!    [`Event`] and pushed down an `mpsc` channel; the *main thread* owns the
//!    connection state machine in [`run`]. Logic lives where it can be read
//!    top-to-bottom, and the C stack is never blocked by ours.
//! 2. **The audio callback is the exception.** `SourceData` hands us a raw
//!    buffer that must be filled *before returning* — it feeds the SBC
//!    encoder in real time. The synthesizer sits behind an `Arc<Mutex<..>>`;
//!    the callback locks, fills pure-math samples, unlocks. Microseconds.
//!    Anything slow in that path (logging, allocation, a held-up lock)
//!    starves the encoder and you hear dropouts.
//!
//! Calling GAP/A2DP methods *from* the main thread while Bluedroid calls us
//! back is safe: the ESP-IDF Bluetooth APIs post messages to the Bluedroid
//! task rather than doing work in the caller's thread.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;

use esp_idf_svc::bt::a2dp::{
    A2dpEvent, ConnectionStatus, EspA2dp, MediaControlCommand, MediaControlStatus,
};
use esp_idf_svc::bt::gap::{DeviceProp, DiscoveryMode, EspGap, GapEvent, InqMode};
use esp_idf_svc::bt::{BdAddr, BtClassic, BtDriver};
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{
    ESP_BT_IO_CAP_NONE, EspError, esp, esp_a2d_media_ctrl, esp_bt_gap_set_security_param,
    esp_bt_gap_ssp_confirm_reply, esp_bt_io_cap_t, esp_bt_sp_param_t_ESP_BT_SP_IOCAP_MODE,
};

use audio::Melody;

/// Both services borrow the one driver; `Arc` lets them share it without
/// lifetime gymnastics (the esp-idf-svc types accept any `Borrow<BtDriver>`).
type Driver = Arc<BtDriver<'static, BtClassic>>;

/// What the Bluedroid callbacks report up to the main thread. Only `Copy`
/// data crosses the channel — the events' borrowed payloads (device props,
/// names) die when the callback returns, so anything needed later (the
/// address, the numeric compare value) is copied out here.
#[derive(Debug)]
enum Event {
    SpeakerFound(BdAddr),
    DiscoveryFinished,
    PairingConfirmRequest(BdAddr),
    Connected(BdAddr),
    Disconnected(BdAddr),
    MediaAck(MediaControlCommand, MediaControlStatus),
    AudioStarted,
}

// --- Binding gaps: raw C calls the safe wrapper doesn't (yet) expose ------
//
// esp-idf-svc (0.52) wraps almost everything we need, with two holes; the
// `sys` module always exposes the full C API precisely so gaps like these
// never block. `esp!` converts a C error code into a Rust `Result`.

/// Source-side stream control (ready-check, start, stop). The safe wrapper
/// has the `MediaControlCommand` enum but no method that *sends* one.
fn media_ctrl(cmd: MediaControlCommand) -> Result<(), EspError> {
    esp!(unsafe { esp_a2d_media_ctrl(cmd as u32) })
}

/// SSP (Secure Simple Pairing) calls. esp-idf-svc gates its safe versions
/// behind `esp_idf_bt_ssp_enabled`, a Kconfig switch that ESP-IDF 5.3
/// REMOVED (SSP is now always on for Classic) — so the methods compile away
/// and we call the C API the same way they would have.
fn set_io_cap_none() -> Result<(), EspError> {
    let io_cap: esp_bt_io_cap_t = ESP_BT_IO_CAP_NONE as _;
    esp!(unsafe {
        esp_bt_gap_set_security_param(
            esp_bt_sp_param_t_ESP_BT_SP_IOCAP_MODE,
            &io_cap as *const _ as *mut core::ffi::c_void,
            1,
        )
    })
}

fn ssp_confirm(bd_addr: &BdAddr, accept: bool) -> Result<(), EspError> {
    esp!(unsafe { esp_bt_gap_ssp_confirm_reply(bd_addr as *const _ as *mut _, accept) })
}

/// Bring up Bluetooth Classic and stream `melody` to the speaker named
/// `speaker_name`, forever (reconnecting whenever the link drops).
pub fn run(
    // `'static` because the driver (and through it, the subscribe closures
    // running on Bluedroid's task) must be allowed to live forever; the
    // singleton `Peripherals::take()` hands out exactly that.
    modem: Modem<'static>,
    speaker_name: &'static str,
    melody: Arc<Mutex<Melody<'static>>>,
) -> anyhow::Result<()> {
    // NVS (non-volatile storage) holds Bluedroid's bond table: pairing keys
    // survive reboots, so a speaker paired once reconnects without being put
    // back into pairing mode.
    let nvs = EspDefaultNvsPartition::take()?;
    let driver: Driver = Arc::new(BtDriver::new(modem, Some(nvs))?);

    let gap = EspGap::new(driver.clone())?;
    let a2dp = EspA2dp::new_source(driver.clone())?;

    let (tx, rx) = mpsc::channel::<Event>();

    // --- GAP callback: discovery + pairing events -> channel ---------------
    let gap_tx = tx.clone();
    gap.subscribe(move |event| match event {
        GapEvent::DeviceDiscovered { bd_addr, props } => {
            // A device may advertise its name as a plain BdName property or
            // packed inside EIR (Extended Inquiry Response) — many speakers
            // only send the latter, so check both.
            let name_matches = props.iter().any(|prop| match prop.prop() {
                DeviceProp::BdName(name) => name == speaker_name,
                DeviceProp::Eir(eir) => {
                    // (The turbofish is an upstream API wart: these generic
                    // parameters are unused by local_name, but unconstrained
                    // generics still must be pinned to *something*.)
                    eir.local_name::<BtClassic, &BtDriver<'_, BtClassic>>() == Some(speaker_name)
                }
                _ => false,
            });
            if name_matches {
                let _ = gap_tx.send(Event::SpeakerFound(bd_addr));
            } else {
                // Logging every discovery is deliberate: watching the serial
                // monitor list your neighborhood's devices is the first
                // visible sign the radio works.
                log::info!("discovered {bd_addr} (not `{speaker_name}`)");
            }
        }
        GapEvent::DeviceDiscoveryStopped => {
            let _ = gap_tx.send(Event::DiscoveryFinished);
        }
        // SSP "numeric comparison": both sides display a number and ask
        // "same?". With no screen on either a speaker or this board, the
        // main thread just answers yes (this is what "Just Works" pairing
        // means — convenient, but it can't detect a man-in-the-middle).
        GapEvent::PairingUserConfirmationRequest { bd_addr, number } => {
            log::info!("pairing confirmation for {bd_addr} (number {number})");
            let _ = gap_tx.send(Event::PairingConfirmRequest(bd_addr));
        }
        GapEvent::AuthenticationCompleted { status, .. } => {
            log::info!("authentication completed: {status:?}");
        }
        _ => (),
    })?;

    // --- A2DP callback: stream events -> channel; audio filled in place ----
    let a2dp_tx = tx.clone();
    let source = melody.clone();
    a2dp.subscribe(move |event| match event {
        // THE audio hot path. Fill the buffer, report how many bytes we
        // wrote, touch nothing else — see the module docs.
        A2dpEvent::SourceData(buf) => {
            audio::fill_bytes(&mut *source.lock().unwrap(), buf);
            buf.len()
        }
        A2dpEvent::ConnectionState {
            bd_addr, status, ..
        } => {
            match status {
                ConnectionStatus::Connected => {
                    let _ = a2dp_tx.send(Event::Connected(bd_addr));
                }
                ConnectionStatus::Disconnected => {
                    let _ = a2dp_tx.send(Event::Disconnected(bd_addr));
                }
                _ => (), // Connecting / Disconnecting are transient
            }
            0
        }
        A2dpEvent::MediaControlAcknowledged { command, status } => {
            let _ = a2dp_tx.send(Event::MediaAck(command, status));
            0
        }
        A2dpEvent::AudioState { status, .. } => {
            if status == esp_idf_svc::bt::a2dp::AudioStatus::Started {
                let _ = a2dp_tx.send(Event::AudioStarted);
            }
            0
        }
        _ => 0,
    })?;

    // Let the speaker's phone-app etc. see us too, then start looking.
    // Inquiry runs ~1.28 s units; 10 units ≈ 13 s per scan round.
    gap.set_device_name("blinky")?;
    // "IO capability: none" = we have no screen or buttons for pairing,
    // which switches SSP into Just-Works mode (see the callback above).
    set_io_cap_none()?;
    gap.set_scan_mode(true, DiscoveryMode::Discoverable)?;
    log::info!("scanning for `{speaker_name}` (put it in pairing mode)...");
    gap.start_discovery(InqMode::General, 10, 0)?;

    // --- The state machine: plain sequential Rust on the main thread -------
    // Target flow (mirrors ESP-IDF's C a2dp_source example):
    //   found -> connect -> connected -> CheckSourceReady -> ack(Success)
    //         -> Start -> AudioStarted -> SourceData callbacks pull samples
    let mut searching = true;
    for event in rx {
        log::debug!("event: {event:?}");
        match event {
            Event::SpeakerFound(addr) => {
                if searching {
                    searching = false;
                    log::info!("found `{speaker_name}` at {addr}, connecting");
                    gap.stop_discovery()?;
                    a2dp.connect_source(&addr)?;
                }
            }
            // Inquiry timed out without a match: scan again. Speakers only
            // answer while in pairing mode, so this loop is normal until
            // the user holds the pairing button.
            Event::DiscoveryFinished => {
                if searching {
                    log::info!("`{speaker_name}` not found yet, rescanning...");
                    gap.start_discovery(InqMode::General, 10, 0)?;
                }
            }
            Event::PairingConfirmRequest(addr) => {
                ssp_confirm(&addr, true)?;
            }
            Event::Connected(addr) => {
                log::info!("connected to {addr}, checking source readiness");
                media_ctrl(MediaControlCommand::CheckSourceReady)?;
            }
            Event::MediaAck(command, status) => match (command, status) {
                (MediaControlCommand::CheckSourceReady, MediaControlStatus::Success) => {
                    media_ctrl(MediaControlCommand::Start)?;
                }
                (MediaControlCommand::CheckSourceReady, _) => {
                    // Not ready yet (Busy shows up right after connecting):
                    // just ask again. The round-trip through the Bluedroid
                    // task is our natural retry pacing.
                    media_ctrl(MediaControlCommand::CheckSourceReady)?;
                }
                (cmd, status) => log::info!("media control {cmd:?} -> {status:?}"),
            },
            Event::AudioStarted => {
                log::info!("streaming — you should hear the melody now");
            }
            Event::Disconnected(addr) => {
                log::warn!("{addr} disconnected, rescanning");
                searching = true;
                gap.start_discovery(InqMode::General, 10, 0)?;
            }
        }
    }

    // All senders live inside the subscribe closures, which live as long as
    // `gap`/`a2dp` — the loop above never actually ends.
    Err(anyhow::anyhow!("bluetooth event channel closed"))
        .context("all event senders dropped unexpectedly")
}
