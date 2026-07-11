# 0003 — Memory sizing: heap, socket slots, and TCP buffers

**Status:** accepted (2026-07-10)

## Context

On a desktop, memory sizes are somebody else's problem. On an ESP32
(~520 KiB RAM total, minus what the radio blob and stacks take), every
buffer is a deliberate choice, and *where* a buffer lives matters as much as
how big it is. Three numbers in this template needed choosing, and the
original code had one real footgun worth recording.

## The numbers

### Heap: `esp_alloc::heap_allocator!(size: 96 * 1024)`

`no_std` Rust has no heap unless you provide one. The main customer here is
**esp-radio** (the WiFi driver), which allocates its internal buffers
dynamically; its documented floor is ~64 KiB depending on config. 96 KiB
gives it comfortable headroom plus room for small app allocations (e.g. the
`String` password in `wifi.rs`). If you add TLS or large payloads, raise it;
if you drop WiFi entirely, most of it can go.

### Socket slots: `StackResources<3>` (`NUM_SOCKETS` in `wifi.rs`)

embassy-net statically reserves one slot per *concurrent* socket:
DHCP client (1) + DNS query (1) + the HTTP client's TCP connection (1) = 3.
It's exactly tight on purpose — each slot costs static RAM. Opening a fourth
concurrent socket without bumping this fails at runtime, so the constant is
documented where it's declared.

### TCP/header buffers: 1 KiB each (`crates/http-client/src/lib.rs`)

Originally `TcpClientState<1, 4096, 4096>` plus a 4096-byte header buffer —
all declared as **locals inside an async fn**. That's the footgun:

> A local in an async fn isn't on "the stack" the way a normal local is.
> The compiler stores it *inside the Future object itself*, so every byte
> of local buffer inflates the future. For spawned Embassy tasks that
> memory is reserved statically per task; for a deeply awaited call chain
> it lands on the executor's real stack. Either way, ~12 KiB hiding inside
> one function is a lot on a microcontroller, and stack overflows on
> bare metal corrupt memory silently rather than failing cleanly.

## Options considered

1. Keep 4 KiB buffers but move them to `static` storage (`static_cell`) —
   right answer for big payloads, but makes `get_status` awkward to call
   more than once (a `StaticCell` initializes exactly once).
2. Shrink to 1 KiB and stay on the async stack — a status-only GET needs to
   read headers, not bodies; 3 KiB total in-future is acceptable and the
   function stays freely reusable.

## Decision

Option 2 for the template (1 KiB tx/rx/header). The rule of thumb recorded
for future work: **buffers ≥ ~2 KiB should live in `static` storage, not in
async fn locals.**

## Consequences

- `get_status` can't receive large responses (fine — it only returns the
  status code). Anything body-heavy needs redesigned buffering, likely
  static, and possibly a bigger `HEADER_BUF` for servers with fat headers.
- Total in-future footprint dropped from ~12 KiB to ~3 KiB.
