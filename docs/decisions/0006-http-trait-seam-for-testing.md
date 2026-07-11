# 0006 — Testing the HTTP client through embedded-nal-async trait seams

**Status:** accepted (2026-07-10)

## Context

After ADR 0005, `get_status` was the only untested logic in the capability
crates (`crates/http-client` sat at 38.89% line coverage). The blocker was
structural, not essential: the function took a concrete
`embassy_net::Stack` and constructed `TcpClient`/`DnsSocket` inside itself,
so exercising it meant standing up a real network stack with a real driver
— exactly the hardware dependency the host-test pattern exists to avoid.

For GPIO the escape hatch was a trait (`StatefulOutputPin`, ADR 0005's
`FakePin`). Networking needed its equivalent — and it already existed:
reqwless's `HttpClient` is generic over `TcpConnect + Dns` from
**embedded-nal-async**, the networking analogue of embedded-hal. Our own
code was the thing erasing that flexibility, by pinning the concrete
embassy-net types earlier than necessary.

## Options considered

1. **Leave it untested.** The "keep boards thin" argument doesn't apply —
   this is portable logic in `crates/`, precisely the bucket the coverage
   ratchet (ADR 0005) exists to protect. Every future network feature would
   pile into the untested corner.
2. **Trait seam via embedded-nal-async generics.** Split the function:
   `get_status_with<T: TcpConnect, D: Dns>` holds the logic;
   `get_status(stack, ..)` becomes a dumb wrapper constructing the
   embassy-net types and delegating. Costs zero firmware bytes
   (monomorphization compiles the generic down to the same code), adds one
   pure-trait dependency that was already in the tree via reqwless and
   embassy-net, and board call sites don't change. Trade-offs: the ~6
   wrapper lines stay uncovered, and the fakes encode assumptions about
   reqwless's I/O pattern (mitigated: assumptions were verified against the
   reqwless 0.14 sources and are pinned by a request-bytes assertion).
3. **std-based mock HTTP server** (spin up a real listener in tests).
   Tests real sockets but not our code path: embassy-net types can't speak
   to a host OS socket, so this would *still* require the seam — plus port
   allocation and timeouts, the classic recipe for flaky CI.
4. **Integration test on hardware.** Highest fidelity, but not automatable
   in CI (ADR 0004): needs live WiFi and a reachable server. That's manual
   flash-testing territory, not the unit suite.

## Decision

Option 2. Supporting choices:

- **Fakes are hand-rolled** (`FakeTcp`, `FakeConn`, `FakeDns` in the test
  module of `crates/http-client/src/lib.rs`), per the repo convention from
  ADR 0005 — no mocking crate. The fake connection serves a canned HTTP
  response and logs written bytes; the test asserts the exact request line
  and Host header that went over the "wire".
- **`embedded-nal-async` becomes a regular dependency** (not dev): the
  trait bounds appear in library code. It's a pure-trait, hardware-agnostic
  crate, so it respects ADR 0001's portability rule.
- **`#[cfg(test)] extern crate std;` in lib.rs**: the test binary links std
  regardless (that's what makes host testing work); this line just admits
  it so test fakes can use `Vec`/`String` instead of contorting around
  fixed buffers. The library itself remains fully `no_std`.
- **Errors:** the fake TCP side uses `Infallible` (embedded-io implements
  its `Error` trait for it); the fake DNS side uses a unit struct, since
  `Dns::Error` only requires `Debug`.
- **The wrapper must stay dumb.** Any logic added to `get_status` itself
  silently escapes the tests; new behavior belongs in `get_status_with`.

## Consequences

- Line coverage of the HTTP client went 38.89% → 86.78%; the CI floor
  ratcheted 75 → 85 (ADR 0005's rule applied).
- The tests verify real behavior, not mocks-of-mocks: DNS resolution is on
  the request path (reqwless resolves every request, even IP literals),
  port selection from the URL scheme, the exact request bytes, status
  propagation, and the DNS-failure error path through a genuine `?`.
- Future network logic (POST bodies, retries, response parsing) is born
  testable — write it in the generic core, test it with the same fakes.
- The fakes track reqwless's I/O contract; a major reqwless upgrade may
  require adjusting them (the request-bytes test will say so loudly).
- Two functions where there was one: callers choose between the embassy-net
  convenience wrapper and the generic core. For boards nothing changes.
