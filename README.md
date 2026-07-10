# blinky — an Embassy template for embedded Rust

A baseline project for async embedded Rust with [Embassy](https://embassy.dev),
structured so the same application logic runs on different chips (ESP32 today,
ARM boards later). The example app blinks an LED, joins WiFi, and makes an
HTTP request.

## Layout

```
app/               Chip-agnostic logic (no_std lib). Blink + HTTP client.
                   Depends only on embassy-* and embedded-hal traits —
                   never on a specific HAL.
boards/esp32/      Everything ESP32-specific: hardware bring-up, WiFi
                   driver, toolchain + cargo config, flashing setup.
                   Self-contained crate with its own Cargo.lock.
docs/decisions/    Short records of why things are the way they are.
                   Start here to understand the structure:
                     0001  why app/ and boards/ are separate crates
                     0002  why board crates aren't workspace members
                     0003  how the memory numbers were chosen
                     0004  why rust-analyzer needs explicit config here
                     0005  how unit tests + the coverage floor work
                     0006  how the HTTP client is tested (trait seam)
.vscode/           Editor config (VS Code and Cursor) so rust-analyzer
                   analyzes for the chip target instead of the host.
CLAUDE.md          Instructions for AI-assisted sessions in this repo.
```

There is intentionally no root `Cargo.toml` — you always build from inside a
board directory so that board's toolchain and target config apply
(see [docs/decisions/0002](docs/decisions/0002-standalone-board-crates.md)).

## Prerequisites (ESP32 board)

ESP32 is an Xtensa chip, which mainline Rust doesn't target yet — Espressif
ships a forked toolchain:

```sh
cargo install espup espflash
espup install            # installs the `esp` toolchain + writes ~/export-esp.sh
```

Two gotchas:

- **Every new shell needs `source ~/export-esp.sh`** before building — it puts
  the Xtensa linker (`xtensa-esp32-elf-gcc`) on your `PATH`. Without it the
  build fails at the link step. Consider adding it to your shell profile.
- **macOS + DevKitC-32E**: the board's USB-serial chip is a CH340, which needs
  the [WCH driver](https://www.wch-ic.com/downloads/CH34XSER_MAC_ZIP.html);
  the device then shows up as `/dev/tty.wchusbserial-*`.

## Editor setup

Open the **repo root** in VS Code or Cursor with the rust-analyzer extension;
the committed [.vscode/settings.json](.vscode/settings.json) makes
rust-analyzer analyze for the ESP32 target with the `esp` toolchain. Without
it, rust-analyzer assumes your host target — where `std` exists — and misses
`no_std` errors. Details in
[docs/decisions/0004](docs/decisions/0004-rust-analyzer-editor-config.md).

## Build, flash, run

```sh
cd boards/esp32
cp .env.example .env     # first time only — fill in your WiFi credentials
cargo run --release      # build + flash via espflash + open serial monitor
```

Credentials in `.env` are injected at **compile time** by `build.rs`
(`env!("WIFI_SSID")`), so they live in the binary — fine for hobby projects,
worth knowing before you share a flashed device. `.env` is gitignored.

Expected serial output: WiFi association, an IP from DHCP, `HTTP status: 200`,
and LED toggle logs every 500 ms. Note: the DevKitC-32E has **no user LED** on
board — wire an LED (with a resistor) to GPIO2, or change the pin in
`boards/esp32/src/main.rs`.

## Testing

Portable logic in `app/` is unit-tested **on the host** — no hardware, no
forked toolchain:

```sh
cd app
cargo test               # plain stable Rust; runs in milliseconds
cargo llvm-cov           # coverage report (cargo install cargo-llvm-cov)
```

This works even though `app` is `no_std`: the test binary runs on your
machine and links `std`; only the library avoids it. Async code is tested
without an executor by polling futures manually and advancing embassy-time's
`MockDriver` — see the test module in [app/src/blink.rs](app/src/blink.rs)
for the pattern (it's the template for new tests). Async **I/O** is tested
the same way with fake network traits — see the test module in
[app/src/http.rs](app/src/http.rs) and
[docs/decisions/0006](docs/decisions/0006-http-trait-seam-for-testing.md).

The board crate has no tests — its binaries are Xtensa machine code that
can't run on a dev machine or CI runner, which is exactly why logic lives in
`app/`. CI enforces a line-coverage floor on `app` (`cargo llvm-cov
--fail-under-lines`); the number is measured, not aspirational — details in
[docs/decisions/0005](docs/decisions/0005-host-unit-tests-and-coverage.md).

## Using this as a template

Knobs to turn first: `SSID`/`PASSWORD` (via `.env`), `HTTP_URL`, the LED pin
and `BLINK_INTERVAL_MS` — all in `boards/esp32/src/main.rs`.

Where new code goes:

- **Portable logic** (protocols, state machines, business logic) → `app/`,
  written against `embedded-hal` traits and `embassy-net`. If it needs an
  `esp-*` crate, it isn't portable — it goes in the board crate.
- **Hardware specifics** (pins, drivers, radios, power) → `boards/<board>/`.

## Porting to a new chip (e.g. RP2040, STM32)

1. Create `boards/<chip>/` as a sibling of `boards/esp32/` — its own
   `Cargo.toml` (`app = { path = "../../app" }`), `src/main.rs`,
   `.cargo/config.toml`, and (if needed) `rust-toolchain.toml` / `memory.x`.
   ARM chips build on stable Rust with `probe-rs` as the runner — no forked
   toolchain needed.
2. In `main.rs`, do that chip's bring-up (its HAL init + embassy setup) and
   wrap the generic fns from `app` in concrete `#[embassy_executor::task]`s,
   exactly like `blink_task` in the ESP32 board.
3. Replace `wifi.rs` with that board's network driver (or drop networking).
   Only the resulting `embassy_net::Stack` crosses into `app`.
4. `app/` is reused unchanged — that's the point of the split.
