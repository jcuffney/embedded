# 0002 — Board crates are standalone, not cargo-workspace members

**Status:** accepted (2026-07-10); amended by
[0007](0007-workspace-and-capability-crates.md) (2026-07-10) — the revisit
condition below fired: a root **host-side** workspace now exists, with board
crates explicitly `exclude`d. Everything this record says about *boards*
still holds.

## Context

Given the `app`/`boards` split (ADR 0001), how should the crates relate?
The reflexive answer is a cargo workspace with every crate as a member. But
cross-compiling for wildly different chips strains three mechanisms that are
all **directory-scoped**:

- `rust-toolchain.toml` — ESP32 is Xtensa and needs Espressif's `esp`
  toolchain fork; ARM chips build on stable. rustup picks the file by
  walking up from the *current directory*.
- `.cargo/config.toml` — sets the default target (`xtensa-esp32-none-elf`
  vs `thumbv7em-...`), the runner (`espflash` vs `probe-rs`), and
  `build-std` (needed for Xtensa, not for ARM). Also resolved from the
  current directory upward.
- `Cargo.lock` — a workspace has exactly one, shared by all members.

## Options considered

1. **One workspace, all crates members.** Breaks in practice: a single root
   toolchain/config can't serve two chips, `cargo build` at the root tries
   to build every board with the wrong target, and one lockfile must
   simultaneously resolve esp-hal's and (later) stm32's dependency trees —
   an upgrade for one board churns the other's lock.
2. **Workspace with boards `exclude`d.** Works, but the root workspace then
   contains only `app`, which is also built as a path dependency of each
   board anyway — the workspace adds a file without adding value yet.
3. **No shared workspace: each board crate is its own root.** Boards depend
   on `app` via `path = "../../app"`. Each board owns its lockfile,
   toolchain file, and cargo config. This is how the Embassy repo organizes
   its per-chip examples, for exactly these reasons.

## Decision

Option 3. There is intentionally **no root `Cargo.toml`**. You build from
inside a board directory (`cd boards/esp32 && cargo run --release`), which
is what makes that board's toolchain and cargo config apply.

## Consequences

- Adding an ARM board later is additive: `boards/rp2040/` with its own
  config files; nothing existing changes.
- Per-board lockfiles mean reproducible, independent builds per chip.
- Cost: `cargo build` at the repo root does nothing (there's no crate
  there), shared dependencies like `embassy-net` are version-pinned in two
  places (`app` and each board) and can drift, and `app` has no lockfile of
  its own — it's always resolved in the context of a board build. If a
  host-side tools crate or `app` unit tests appear later, revisit option 2.
