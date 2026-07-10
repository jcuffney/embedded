# 0004 — CI on GitHub Actions: two jobs, standard runners

**Status:** accepted (2026-07-10)

## Context

Nothing verified pushes or PRs: a change could break the build for the
chip, fail lints, or break `app`'s host-side tests without anyone noticing
until the next flash. We want CI on standard GitHub-hosted runners
(`ubuntu-latest`) — no self-hosted hardware.

Two properties of this repo shape the design:

- The crates are **not** a workspace (ADR 0002); each is built from inside
  its own directory so its `rust-toolchain.toml` / `.cargo/config.toml`
  apply. CI has to mirror that with per-job `working-directory`.
- `boards/esp32` targets `xtensa-esp32-none-elf`. Upstream rustc has no
  Xtensa backend, so the runner needs Espressif's `esp` toolchain fork —
  and whatever it builds is Xtensa machine code that an x86 runner cannot
  execute.

## Options considered

1. **One job that builds everything.** Simple, but forces the slow esp
   toolchain install onto the fast `app` checks, and `working-directory`
   gymnastics obscure which crate failed.
2. **Two independent jobs, one per crate.** Mirrors the repo layout: `app`
   runs on plain `dtolnay/rust-toolchain@stable` (fmt, clippy, test);
   `esp32` installs the fork and does fmt, clippy, release build. Jobs run
   in parallel; a failure names its crate.
3. **Hand-rolled `espup` install for the esp job.** More control, but
   re-implements what `esp-rs/xtensa-toolchain` (the official action,
   `espup` under the hood) already maintains — including toolchain caching
   quirks and export-file sourcing.

## Decision

Option 2, using `esp-rs/xtensa-toolchain@v1.7` (`ldproxy: false` — ldproxy
is only for ESP-IDF/`std` projects, this is bare-metal `no_std`).
Supporting choices:

- **Dummy `.env` in CI** (`cp .env.example .env`): `main.rs` reads WiFi
  credentials at compile time via `env!()`, so the build fails without one.
  Placeholder values are safe — CI never flashes, and real credentials stay
  out of the repo and out of CI entirely.
- **No on-target tests**: the esp32 job stops at `cargo build --release`.
  Running Xtensa code needs hardware or QEMU; if that's ever wanted,
  Espressif's QEMU fork + `probe-rs`/Wokwi-style runners deserve their own
  ADR.
- **`-D warnings` on clippy**: warnings rot when they can accumulate; CI
  treats them as errors so they're fixed in the PR that introduces them.
- **`Swatinem/rust-cache`**: the esp32 job compiles `core`/`alloc` from
  source (`build-std`) on every cold run; caching cuts warm runs from
  many minutes to roughly one.
- **Concurrency cancellation** keyed on branch: a force-push obsoletes the
  previous commit's run, so it's cancelled instead of finishing.

## Consequences

- Every PR proves: `app` is formatted, lint-clean, and passes host tests;
  `boards/esp32` is formatted, lint-clean, and cross-compiles for the chip.
- Lints are now load-bearing: code that merely *warned* locally will fail
  CI until fixed.
- Cost: the esp32 job's cold start (toolchain install + build-std) is
  ~5–10 minutes; nothing exercises the binary's runtime behavior — CI
  proves it compiles, not that it blinks.
- A future board (`boards/rp2040/`, ARM) is additive here too: copy the
  esp32 job, swap the toolchain action for `dtolnay/rust-toolchain` with
  `targets: thumbv...`, drop the `.env` step if unneeded.
