# 0005 — Host unit tests with mock time, and a measured coverage floor

**Status:** accepted (2026-07-10)

## Context

The whole point of the `crates/`/`boards/` split (ADR 0001) is that
portable logic can be exercised without hardware — yet the capability
crates had zero tests, and CI's `cargo test` step passed vacuously. Nothing
stopped logic bugs from reaching the flash step, and nothing stopped test
coverage from silently eroding as code grew.

Testing these crates is not entirely conventional, which is why the pattern
deserves an ADR:

- The capability crates are `#![no_std]`, so it's easy to assume
  `cargo test` can't work. It can: the **test binary** runs on the host and
  links `std`; only the library itself avoids it.
- The interesting function, `blink`, is `async` and returns `!` — no
  executor can ever "finish" it, and real time (`Timer::after`) would make
  tests slow and flaky.
- Its hardware side is a trait (`StatefulOutputPin`), which needs a test
  double.
- On hardware, `critical-section` and the embassy timer queue are provided
  by the board crate (esp-hal / esp-rtos). A host test binary has neither
  and fails to *link* unless the test build supplies substitutes.

## Options considered

1. **Executor-based async tests** (embassy-executor on the host, or
   tokio + `#[tokio::test]`). Heavy dev-dependencies, and they don't solve
   the core problem: a `-> !` future never completes, so the test would
   still need to race it against a timeout — timing-dependent, the thing
   unit tests should never be.
2. **Manual polling + `MockDriver` + a hand-rolled `FakePin`.** Do what an
   executor does, by hand: poll once, advance a fake clock, poll again.
   embassy-time's `mock-driver` feature exists for exactly this; a fake pin
   is ~30 lines of trait impl. Zero new runtime deps, fully deterministic,
   and the test doubles as a lesson in how async actually works.
3. **`embedded-hal-mock` for the pin.** A maintained crate, but its
   expectation-style mocks are a dependency for something 30 lines teach
   better here.

For coverage tooling:

1. **cargo-llvm-cov** — LLVM source-based instrumentation (the compiler's
   own coverage), accurate region/line data, actively maintained, one-line
   threshold flag, prebuilt CI action.
2. **cargo-tarpaulin** — historically x86-Linux-focused with ptrace-based
   quirks; less precise.

For the threshold style: an **aspirational** number (e.g. "80% because it
sounds right") starts life red or forces test-padding; a **measured floor**
(what the suite actually covers, rounded down) starts green and acts as a
ratchet against regression.

## Decision

Option 2 for tests, cargo-llvm-cov with a measured floor for coverage:

- **Inline `#[cfg(test)] mod tests`** next to the code under test — the
  repo's pattern for unit tests (integration `tests/` dirs can come later
  if something warrants one).
- **`[dev-dependencies]` provide the host substitutes**: `embassy-time`
  re-declared with `mock-driver` (cargo unifies features, test builds only),
  `critical-section` with its `std` implementation, and
  `embassy-time-queue-utils` with `generic-queue-8` (a self-contained timer
  queue that needs no executor). Dev-deps are invisible to downstream
  crates, so none of this can leak into firmware builds — `boards/esp32`'s
  separate lockfile makes that doubly true.
- **Manual polling** with `core::pin::pin!` + `Waker::noop()` (stable
  since Rust 1.85), advancing time with `MockDriver::get().advance()`.
  Caveat: `MockDriver` is one process-global clock and tests run on
  parallel threads, so time-advancing assertions stay within a single test
  fn until something forces a serialization scheme.
- **CI**: the host job's test step is
  `cargo llvm-cov --workspace --fail-under-lines N`, which runs the same
  suite instrumented and exits non-zero below the floor. N is **measured**
  — what the suite actually covers, rounded down to the nearest 5 (75 when
  tests were introduced; 85 today, after the HTTP trait seam of ADR 0006).
  Raising it belongs in the same PR as the tests that earn it; never lower
  it to make a PR pass.

## Consequences

- Logic bugs in portable code are now caught on the host in milliseconds,
  before any flash cycle; PRs that drag line coverage below the floor fail
  CI.
- The pattern is documented by example: the test module in
  `crates/blink/src/lib.rs` is the template for testing async + trait-based
  code (FakePin, mock clock, manual polls); `crates/http-client`'s test
  module for network logic through trait-seam fakes (ADR 0006).
- Test code is clippy-load-bearing (`--all-targets -D warnings` covers it).
- `boards/esp32` still has no runtime verification (CI proves it compiles,
  not that it blinks). On-target testing (`embedded-test`, QEMU, HIL) is a
  future ADR.
- The floor is a ratchet, not a guarantee: a small amount of new untested
  code can still merge if the total stays above the floor. Stricter gates
  (e.g. per-PR diff coverage) were deliberately skipped as overkill for a
  template.
