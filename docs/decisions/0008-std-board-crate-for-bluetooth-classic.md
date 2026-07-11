# 0008 — A `std` (ESP-IDF) board crate for Bluetooth Classic audio

**Status:** accepted (2026-07-10)

## Context

We want the ESP32 to connect to a Bluetooth speaker and play audio. The chip
can: the original ESP32 is dual-mode, supporting both **BLE** (Bluetooth Low
Energy, 2010, designed for tiny intermittent messages) and **Bluetooth
Classic** (BR/EDR, 1999, designed as a continuous cable-replacement pipe).
Speakers only accept audio via **A2DP** — a *Classic* profile stacked on
L2CAP → SDP → AVDTP, with SBC-encoded PCM at ~330 kbit/s sustained. A2DP is
the only protocol consumer speakers speak; BLE cannot carry it (LE Audio
exists, but neither this chip nor common speakers support it).

The problem: all of that profile machinery is a **host stack** — software —
and the only implementation for ESP32 is **Bluedroid**, Espressif's port of
Android's Bluetooth stack: a large C codebase that requires FreeRTOS threads
and a heap, i.e. it only runs inside **ESP-IDF**. Our `no_std` stack
(esp-hal + esp-rtos + esp-radio) exposes Wi-Fi and BLE only; no `no_std`
Bluetooth Classic host exists in Rust for any chip, and none is likely
(BR/EDR is a legacy protocol in maintenance mode). "Ships with WiFi and
Bluetooth" was a statement about silicon, not about what any given software
stack lets you reach.

The Rust path that *does* work: the `xtensa-esp32-espidf` target runs Rust
`std` on top of ESP-IDF (FreeRTOS provides threads, newlib provides
malloc/libc), and **esp-idf-svc** wraps Bluedroid — including A2DP *source*
mode (`EspA2dp::new_source`, a `SourceData(&mut [u8])` callback that asks
the app to fill raw PCM; the stack does SBC encoding and the radio work).

## Options considered

1. **Wait for `no_std` Bluetooth Classic.** Keeps one toolchain, but the wait
   is indefinite and probably forever — rewriting Bluedroid-scale C in Rust
   for a legacy protocol has no ecosystem momentum.
2. **Convert the existing board crate to `std`/ESP-IDF.** One board, one
   flavor — but it discards the bare-metal learning track (esp-hal, Embassy
   on metal, explicit memory budgets from ADR 0003) and churns working code
   for a feature unrelated to it.
3. **BLE audio.** Stays `no_std`, but the original ESP32 lacks the LE Audio
   radio features (BLE 5.2 isochronous channels) and almost no consumer
   speakers support LE Audio anyway. Dead end on both sides of the link.
4. **A new sibling `std` board crate just for this capability.** ADR 0001/0002
   already make boards standalone crates with their own toolchains; a board
   whose "toolchain" includes an RTOS fits the existing shape. Costs: two
   Rust flavors in one repo, a ~1 GB ESP-IDF download and slow first build,
   a C stack underneath Rust's safety guarantees, and esp-idf-svc's bt API
   is marked experimental (signature churn risk).

## Decision

Option 4: `boards/esp32-a2dp/` — a self-contained `std` binary crate on
esp-idf-svc that discovers a speaker by name, pairs, and streams audio as an
A2DP source. The framing rule this sets for the template: **each board crate
picks the smallest runtime that can do its job** — bare metal for GPIO/Wi-Fi
(`boards/esp32`), an RTOS where a required C stack demands one (this board).

The audio itself (tone/melody synthesis) is pure math, so it lives in a new
portable capability crate `crates/audio` (ADR 0001 boundary): `no_std`,
dependency-free, host-tested to the ADR 0005 coverage bar. The board crate
only wires its `AudioSource::fill` seam into the A2DP data callback — same
pattern as ADR 0006's network seams.

## Consequences

- Gained: working Bluetooth speaker audio today, plus a worked example of
  the `std`/ESP-IDF flavor for future capabilities that need big C stacks
  (OTA, TLS, filesystems).
- Gained: `crates/audio` is reusable for a future wired (I2S) audio board
  unchanged.
- Gave up: single-toolchain simplicity. The two boards build differently
  (`xtensa-esp32-none-elf` + espflash vs `xtensa-esp32-espidf` + ldproxy),
  and rust-analyzer can only point at one board at a time (ADR 0007).
- Gave up: full-stack transparency on this board — beneath our Rust sit
  FreeRTOS, Bluedroid, and lwIP in C; Rust's memory-safety guarantees stop
  at that FFI line.
- Accepted: heavy builds (first build compiles ESP-IDF, ~10–20 min and
  several GB) and a matching CI job whose cold runs are slow until caches
  warm (see ADR 0004; the job caches `~/.espressif`).
- Accepted: esp-idf-svc's bt module is experimental; if its API churns we
  pin the exact version and absorb signature changes in the board crate,
  where they can't leak past the `AudioSource` seam.
