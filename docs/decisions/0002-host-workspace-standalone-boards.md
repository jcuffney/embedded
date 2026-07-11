# 0002 — Root host workspace; board crates standalone and excluded

**Status:** accepted (2026-07-10)

## Context

Given the split in ADR 0001, the repo holds two kinds of crates with
opposite needs:

- **Capability crates** (`crates/*`) build on **stable host Rust** and carry
  the whole unit-test suite. They want what any multi-crate Rust project
  wants: shared dependency versions, one lockfile, one command that tests
  everything.
- **Board crates** (`boards/*`) cross-compile for wildly different chips,
  which strains three mechanisms that are all **directory-scoped**:
  - `rust-toolchain.toml` — ESP32 is Xtensa and needs Espressif's `esp`
    toolchain fork; ARM chips build on stable. rustup picks the file by
    walking up from the *current directory*.
  - `.cargo/config.toml` — sets the default target (`xtensa-esp32-none-elf`
    vs `thumbv7em-...`), the runner (`espflash` vs `probe-rs`), and
    `build-std` (needed for Xtensa, not for ARM). Also resolved from the
    current directory upward.
  - `Cargo.lock` — a workspace has exactly one, shared by all members.

## Options considered

1. **One workspace, everything a member.** Breaks in practice: a single
   root toolchain/config can't serve two chips, `cargo build` at the root
   tries to build every board with the wrong target, and one lockfile must
   simultaneously resolve esp-hal's and (later) stm32's dependency trees —
   an upgrade for one board churns the other's lock.
2. **No shared workspace at all: every crate is its own root.** This was
   the repo's first shape, and it works while there is exactly one logic
   crate. It stops working as host-side crates multiply: shared versions
   get pinned in several places with nothing to stop drift, host code has
   no lockfile of its own, and there is no single `cargo test` for
   everything testable.
3. **A root *virtual* workspace for the host side, with boards excluded.**
   Host crates get the workspace benefits; board crates keep being their
   own roots (own toolchain file, cargo config, lockfile) — which is how
   the Embassy repo organizes its per-chip examples.

## Decision

Option 3. The root `Cargo.toml` is a **virtual manifest** — a `[workspace]`
table with no `[package]` and no code:

- `members = ["crates/*"]`; `exclude` lists each board. The exclude is
  load-bearing: cargo resolves a crate's workspace by walking *up* the
  directory tree, so without it a board build errors at startup. Note
  `exclude` takes **literal paths, not globs** (unlike `members`) — each
  new board gets listed explicitly.
- `[workspace.dependencies]` declares shared dependency versions once;
  members consume them with `{ workspace = true }` and may add features
  (e.g. the dev-only `mock-driver` on embassy-time).
- Board crates stay standalone: they depend on capability crates via
  `path = "../../crates/<name>"`, and each owns its lockfile, toolchain
  file, and cargo config. Firmware is always built from inside the board
  directory — that is what makes those files apply.

Crate naming: name a crate for the capability it provides (`blink`,
`http-client`), not its layer. `http-client` deliberately avoids the name
`http` — the popular `http` crate is a likely dependency of a future
backend, and two same-named packages cannot coexist in one dependency
graph. Avoid `core` for the same reason (it's Rust's built-in `no_std`
standard-library crate).

## Consequences

- `cargo test --workspace` / `cargo llvm-cov --workspace` run everything
  host-testable from the repo root, against one shared lockfile and target
  directory; CI's host job does the same (ADR 0004).
- Host-side dependency versions can no longer drift between crates.
- Growth is additive: a new capability is a new member crate, a backend
  service is a new member, a new chip is a new (excluded) board —
  `boards/rp2040/` with its own config files; nothing existing changes.
- Per-board lockfiles mean reproducible, independent builds per chip. The
  cost: versions shared between a board and the workspace (e.g.
  `embassy-net`) are still pinned in two places and can drift — the
  workspace only protects the host side.
- `cargo build` at the repo root builds only the host side; boards must be
  built from their own directories, and each new board must also be added
  to the root `exclude` list.
- rust-analyzer keeps pointing at the board crate (ADR 0007), *not* the
  root workspace — analysis should happen for the chip target.
