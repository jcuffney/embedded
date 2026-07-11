# 0001 — Split portable capability crates (`crates/`) from board bring-up (`boards/`)

**Status:** accepted (2026-07-10)

## Context

This repo is meant to be a template reused across chips: ESP32 today, ARM
(STM32, RP2040, nRF, ...) later. In the original single-crate layout,
portable code (the HTTP client) sat next to chip-specific code (`main.rs`,
`wifi.rs`) with nothing preventing ESP-only types from leaking everywhere.
Porting would have meant untangling the crate by hand.

The Rust embedded ecosystem's portability mechanism is **embedded-hal**: a
set of traits (`OutputPin`, `SpiBus`, `I2c`, ...) that every HAL implements.
Code written against the traits runs on any chip; code written against a
concrete HAL type (`esp_hal::gpio::Output`) runs on one. Similarly,
**embassy-net** gives a portable network `Stack` interface over any driver,
and **embedded-nal-async** does the same for client-side networking.

## Options considered

1. **One crate, disciplined modules.** Zero restructuring, but nothing
   *enforces* the boundary — one careless `use esp_hal::...` in shared code
   and portability silently breaks. Also impossible anyway across
   Xtensa/ARM because of toolchains (see ADR 0002).
2. **Feature flags per chip in one crate.** Common in driver crates, but for
   applications it produces `#[cfg]` soup, and the toolchain problem remains.
3. **Library crates for logic + a binary crate per board.** The compiler
   enforces the boundary: the library manifests simply have no HAL
   dependencies, so chip-specific code *cannot* compile there. This is the
   pattern the Embassy project itself uses.

## Decision

Option 3, as **capability crates**: one small `#![no_std]` library per unit
of portable logic (`crates/blink`, `crates/http-client`), each named for the
capability it provides. The boundary rule:

- `crates/` holds **application logic that works regardless of hardware** —
  written only against portable traits (embedded-hal, embedded-nal-async,
  embassy-net/-time interfaces). If code needs an `esp-*`/`stm32-*`
  dependency, it is not portable and does not belong here.
- `boards/` holds **board-specific logic**: hardware bring-up plus the
  concrete trait implementations (pins, drivers, network stack wiring) a
  board needs to be flashed and run. A board binary is where every trait
  seam gets filled in with real hardware types.

One wrinkle worth remembering: `#[embassy_executor::task]` functions are
statically allocated, so the macro **cannot be applied to generic
functions** (the compiler wouldn't know how many concrete instances to
reserve memory for). The pattern that falls out:

- capability crates expose plain **generic async fns** (`blink(pin: &mut
  impl StatefulOutputPin, ...)`),
- each board wraps them in a concrete `#[task]` with its own pin/driver
  types (`blink_task(led: Output<'static>)` in `boards/esp32/src/main.rs`).

## Consequences

- Porting to a new chip = write a new `boards/<chip>/` crate (bring-up +
  drivers); the capability crates are reused unchanged.
- The portability boundary is compiler-checked, not convention-checked.
- Cost: a little indirection (board-side task wrappers), and several small
  crates to version instead of one — how they relate is ADR 0002.
- WiFi control (`wifi.rs`) stays board-side: esp-radio has no portable trait
  layer, and e.g. an STM32 board would use Ethernet or a different radio
  entirely. Only the resulting `Stack` crosses the boundary.
