# 0007 — Root host workspace + `crates/` of capability crates (replaces `app/`)

**Status:** accepted (2026-07-10); amends [0002](0002-standalone-board-crates.md)

## Context

Two pressures converged on the old single `app/` crate:

1. **Semantics.** The name `app` bakes in the assumption that there is
   exactly one application. What the crate actually held was independent
   *units of portable logic* (blink, an HTTP client). Real projects built
   from this template will have multiple firmware roles (e.g. a sensor node
   reporting to a backend and an actuator node driven by an API) plus
   host-side crates (shared wire types, backend services) — a monolithic
   "app" has no honest place for any of that.
2. **ADR 0002's revisit trigger fired.** That record rejected a root
   workspace but explicitly said to revisit once host-side crates or unit
   tests appeared. They have: the portable crates carry the whole test
   suite, and they build on **stable host Rust** — none of the
   toolchain/target/lockfile conflicts that motivated 0002 apply to them.
   Meanwhile the costs 0002 predicted were being paid: versions pinned in
   multiple places with nothing to stop drift, and no lockfile or single
   test command for host code.

## Options considered

1. **Keep the `app` monolith.** Zero churn, but the misleading name remains
   and every future capability lands in one crate that only gets harder to
   split.
2. **Rename in place (e.g. `blinky-core`).** Fixes the name, changes
   nothing structurally: still one monolith, still no workspace, still
   nowhere for a second firmware role or a backend crate to go.
3. **`crates/` directory of small capability crates + a root *virtual*
   workspace.** The standard Rust monorepo shape (Embassy itself uses it).
   Growth becomes additive: a new role is a new crate, a backend is a new
   member, a new chip is a new (excluded) board. Costs: more manifest
   boilerplate, and boards declare one path dependency per capability crate
   instead of one for everything.

## Decision

Option 3. The old `app` crate is split into `crates/blink` and
`crates/http-client`, and a root `Cargo.toml` — a **virtual manifest**
(a `[workspace]` table, no `[package]`, no code) — ties the host side
together:

- `members = ["crates/*"]`, `exclude = ["boards/esp32"]`. The exclude is
  load-bearing: cargo resolves a crate's workspace by walking *up* the
  directory tree, so without it a board build errors at startup. Note
  `exclude` takes **literal paths, not globs** (unlike `members`) — each new
  board gets listed explicitly.
- `[workspace.dependencies]` declares shared dependency versions once;
  members consume them with `{ workspace = true }` and may add features
  (e.g. the dev-only `mock-driver`).

**The boundary rule** (the same line ADR 0001 drew, restated for the new
layout):

- `crates/` holds **application logic that works regardless of hardware** —
  written only against portable traits (embedded-hal, embedded-nal-async,
  embassy-net/-time interfaces). If code needs an `esp-*`/`stm32-*`
  dependency, it is not portable and does not belong here.
- `boards/` holds **board-specific logic**: hardware bring-up plus the
  concrete trait implementations (pins, drivers, network stack wiring) a
  board needs to be flashed and run. A board binary is where every trait
  seam gets filled in with real hardware types.

Crate naming: name a crate for the capability it provides (`blink`,
`http-client`), not its layer. `http-client` deliberately avoids the name
`http` — the popular `http` crate is a likely dependency of a future
backend, and two same-named packages cannot coexist in one dependency
graph. Avoid `core` for the same reason (it's Rust's built-in `no_std`
standard-library crate).

## Consequences

- `cargo test --workspace` / `cargo llvm-cov --workspace` run everything
  host-testable from the repo root, against one shared lockfile and target
  directory; CI's host job follows suit (coverage floor unchanged at 85 —
  measured 88 after the split).
- Host-side dependency versions can no longer drift between crates.
- Future growth is additive: `crates/contract` (shared wire types),
  `backend/` crates as new members, `infra/` as a plain directory,
  sibling roles as new crates, new chips as new excluded boards.
- Boards are untouched operationally (own toolchain, lock, config; build
  from inside the board dir), but now list one path dependency per
  capability crate they use, and each new board must also be added to the
  root `exclude` list.
- rust-analyzer keeps pointing at the board crate (ADR 0004), *not* the
  root workspace — analysis should happen for the chip target.
