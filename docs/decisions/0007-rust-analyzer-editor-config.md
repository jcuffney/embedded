# 0007 — rust-analyzer configuration for no_std cross-compilation

**Status:** accepted (2026-07-10)

## Context

rust-analyzer was analyzing the code as if `std` were available: no errors on
`std`-only APIs, wrong `cfg` resolution, and inlay info for the wrong
platform. Two separate causes:

1. **Wrong target.** rust-analyzer analyzes code *for a target*, defaulting
   to the host (an aarch64 Mac — which has `std`). Terminal builds get the
   right target from `boards/esp32/.cargo/config.toml`, but that file is
   discovered relative to the directory cargo runs in, and the editor opens
   the repo root.
2. **No discovery of the board crate.** The root workspace contains only
   the host-side capability crates; `boards/esp32` is deliberately excluded
   (ADR 0002). Opened at the repo root, rust-analyzer never finds the
   firmware crate on its own and falls back to per-file analysis with
   default (host) assumptions for it.

There's also a toolchain wrinkle: the `xtensa-esp32-none-elf` target only
exists in Espressif's forked `esp` toolchain. `boards/esp32/
rust-toolchain.toml` selects it for terminal builds, but the rust-analyzer
server process starts at the repo root where no toolchain file applies.

## Options considered

1. **Open `boards/esp32/` directly as the editor workspace.** Everything
   resolves naturally (cwd-based discovery works), but you lose sight of
   `crates/`, `docs/`, and the repo root — bad ergonomics for a template.
2. **Make the board crate a workspace member so root discovery finds it.**
   Rejected in ADR 0002 for toolchain reasons; reversing that to please the
   editor is the tail wagging the dog — and it still wouldn't fix the
   target or toolchain the analysis runs with.
3. **Committed `.vscode/settings.json`** that tells rust-analyzer
   explicitly: which project (`linkedProjects`), which target
   (`cargo.target`), which toolchain (`server.extraEnv.RUSTUP_TOOLCHAIN`),
   and to skip host-target artifacts (`allTargets: false`). Cursor reads
   `.vscode/` too, so one config serves both editors.

## Decision

Option 3 — this mirrors what Espressif's own `esp-generate` template emits,
extended with `linkedProjects` because of our multi-crate layout. Analysis
happens **for the chip**: the board crate pulls in the capability crates as
path dependencies, so they are analyzed with `no_std` + Xtensa semantics
too — the strictest interpretation, which is the one worth checking against.

## Consequences

- rust-analyzer now analyzes with `no_std` semantics and the Xtensa target;
  `std`-only code gets flagged in the editor instead of at build time.
- The settings are committed, so every clone of the template gets a working
  editor out of the box.
- **Caveat for the multi-board future:** `cargo.target` and
  `RUSTUP_TOOLCHAIN` are editor-wide, so rust-analyzer can only analyze for
  one chip at a time. When an ARM board is added, either switch these
  settings while working on it, or use a VS Code *multi-root workspace*
  with per-folder settings (one folder per board). Revisit then.
