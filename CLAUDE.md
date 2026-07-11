# CLAUDE.md

## About the user

I am **learning embedded Rust**. When responding to any question or prompt:

- **Explain like a teacher.** Define terms the first time they appear, explain *why* before *how*, and connect new concepts to code that already exists in this repo.
- Don't just deliver working code — briefly explain what it does and why that approach was chosen.

## Decision records are mandatory

Every key technical decision (architecture, dependency choice, memory sizing, toolchain/config choices, trade-offs) must be documented **at the time it is made** as a short record in `docs/decisions/`, numbered sequentially, with this shape:

1. **Context** — what problem forced a choice
2. **Options considered** — with honest trade-offs
3. **Decision** — what we picked
4. **Consequences** — what we gained and gave up

## Project orientation

This repo is a **template** for embedded Rust (Embassy) projects, structured for multi-chip reuse:

- `crates/` — capability crates: **application logic that works regardless of hardware**, one small chip-agnostic `no_std` library per capability (`blink`, `http-client`, ...). Only portable deps allowed (embassy-*, embedded-hal / embedded-nal-async traits). Never add `esp-*`/`stm32-*` here — if code needs one, it belongs in a board crate.
- `boards/esp32/` — **board-specific logic**: self-contained binary crate (own `Cargo.lock`, `rust-toolchain.toml`, `.cargo/config.toml`). All hardware bring-up lives here, plus the concrete trait implementations that fill in the capability crates' seams so the firmware can actually be flashed. New chips get sibling directories.
- Root `Cargo.toml` — **virtual workspace for host-buildable crates only** (`members = ["crates/*"]`). Boards are `exclude`d (literal paths, not globs — list each new board). See `docs/decisions/0007`.
- `docs/decisions/` — the ADRs described above.

Build/flash workflow (must run from inside the board directory so its toolchain and cargo config apply):

```sh
cd boards/esp32
cp .env.example .env   # first time only; fill in WiFi credentials
cargo run --release    # builds, flashes via espflash, opens serial monitor
```

Secrets live in `boards/esp32/.env` (gitignored, injected at compile time by `build.rs`). Never commit credentials.

## Testing pattern

- New logic in `crates/` gets an inline `#[cfg(test)] mod tests` next to the code, run **on the host** from the repo root: `cargo test --workspace`. (Works despite `no_std` — the test binary links `std`; the libraries don't.)
- Async code: no executor in tests. Poll futures manually (`core::pin::pin!` + `Waker::noop()`) and advance time with `embassy_time::MockDriver` (enabled via `[dev-dependencies]` only). `MockDriver` is a process-global clock — keep time-advancing assertions within one test fn. Template: the test module in `crates/blink/src/lib.rs`.
- Hardware and network traits get hand-rolled fakes (`FakePin` in `crates/blink/src/lib.rs`; `FakeTcp`/`FakeDns` in `crates/http-client/src/lib.rs`), not a mocking crate. Network logic goes in generic cores bounded on `embedded-nal-async` traits (`get_status_with`), with dumb concrete wrappers at the edge — see `docs/decisions/0006`.
- CI enforces a line-coverage floor on the workspace (`cargo llvm-cov --workspace --fail-under-lines N` in `.github/workflows/ci.yml`). N is **measured, rounded down to the nearest 5** — when a PR meaningfully raises coverage, ratchet N up in that same PR. Never lower it to make a PR pass.
- `boards/*` crates get no unit tests (cross-compiled binaries can't run in CI); keep them thin wrappers so logic stays testable in `crates/`. See `docs/decisions/0005`.

Hardware note: the dev board is an ESP32-DevKitC-32E — CH340 USB-serial (WCH driver on macOS, `/dev/tty.wchusbserial-*`), and it has **no user LED**; blinky drives GPIO2 for an external LED.
