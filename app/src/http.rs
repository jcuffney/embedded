//! Minimal HTTP client on top of embassy-net + reqwless.
//!
//! Chip-agnostic: this file only sees the portable `Stack` interface; the
//! chip-specific network driver (WiFi, Ethernet, ...) that feeds it lives in
//! the board crate.

use embassy_net::{
    Stack,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use embedded_nal_async::{Dns, TcpConnect};
use reqwless::{client::HttpClient, request::Method};

// Buffer sizing matters more than usual here: these arrays live on the stack
// of the async fn below, and an async fn's stack is baked into its Future —
// which for a spawned task is statically allocated. 1 KiB each is plenty for
// a status-only GET; bump them if you start reading large bodies, but prefer
// `static` buffers (via static_cell) over big stack arrays.
// See docs/decisions/0003-buffers-heap-and-sockets.md.
const TCP_TX_SZ: usize = 1024;
const TCP_RX_SZ: usize = 1024;
const HEADER_BUF: usize = 1024;

#[derive(Debug)]
#[allow(dead_code)]
pub enum HttpError {
    Request(reqwless::Error),
}

impl From<reqwless::Error> for HttpError {
    fn from(e: reqwless::Error) -> Self {
        HttpError::Request(e)
    }
}

/// The testable core: generic over WHO provides TCP and DNS.
///
/// `TcpConnect` and `Dns` are embedded-nal-async's portability traits — the
/// networking equivalent of embedded-hal's `OutputPin`. On hardware they're
/// implemented by embassy-net's `TcpClient`/`DnsSocket`; in tests, by the
/// hand-rolled fakes below. Same seam, same lesson as `FakePin` in blink.rs:
/// depend on traits, inject the concrete types at the edge. See ADR 0006.
pub async fn get_status_with<T: TcpConnect, D: Dns>(
    tcp: &T,
    dns: &D,
    url: &str,
) -> Result<u16, HttpError> {
    let mut client = HttpClient::new(tcp, dns);

    let mut rx_buf = [0u8; HEADER_BUF];
    let mut req = client.request(Method::GET, url).await?;
    let resp = req.send(&mut rx_buf).await?;
    Ok(resp.status.0)
}

/// Perform a GET request over an embassy-net stack; returns the status code.
///
/// Deliberately a dumb wrapper: it only constructs the real network objects
/// and delegates. Any logic added here silently escapes the unit tests —
/// put it in `get_status_with` instead.
pub async fn get_status(stack: Stack<'static>, url: &str) -> Result<u16, HttpError> {
    // `<1, ...>` = one pooled TCP connection; this client makes one request
    // at a time.
    let state: TcpClientState<1, TCP_TX_SZ, TCP_RX_SZ> = TcpClientState::new();
    let tcp = TcpClient::new(stack, &state);
    let dns = DnsSocket::new(stack);
    get_status_with(&tcp, &dns, url).await
}

#[cfg(test)]
mod tests {
    use core::cell::RefCell;
    use core::convert::Infallible;
    use core::future::Future;
    use core::net::{IpAddr, Ipv4Addr, SocketAddr};
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};

    use embedded_io_async::{ErrorType, Read, Write};
    use embedded_nal_async::{AddrType, Dns, TcpConnect};
    // Available because lib.rs declares `#[cfg(test)] extern crate std` —
    // std types are fine in tests; the library itself never sees them. (No
    // std prelude in a no_std crate, so even Vec/String need explicit
    // imports.)
    use std::string::String;
    use std::vec::Vec;

    use super::{HttpError, get_status_with};

    // -- FakeDns -----------------------------------------------------------

    /// `Dns::Error` only requires `Debug`, so a unit struct is all it takes.
    #[derive(Debug)]
    struct DnsRefused;

    struct FakeDns {
        fail: bool,
    }

    impl Dns for FakeDns {
        type Error = DnsRefused;

        async fn get_host_by_name(
            &self,
            _host: &str,
            _addr_type: AddrType,
        ) -> Result<IpAddr, DnsRefused> {
            if self.fail {
                Err(DnsRefused)
            } else {
                // 192.0.2.0/24 is TEST-NET-1, reserved for documentation
                // and examples — it can never be a real host.
                Ok(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)))
            }
        }

        async fn get_host_by_address(
            &self,
            _addr: IpAddr,
            _result: &mut [u8],
        ) -> Result<usize, DnsRefused> {
            unimplemented!("reqwless never resolves addresses back to names")
        }
    }

    // -- FakeTcp -----------------------------------------------------------

    /// One fake TCP connection: reads serve a canned HTTP response, writes
    /// append to a log the test inspects afterwards.
    ///
    /// Why the references: `TcpConnect::connect(&self)` takes SHARED self
    /// (a real stack hands out many connections), so mutable state lives
    /// per-connection (the read cursor) or behind `RefCell` (the write log)
    /// — the same trick as FakePin's `&Cell`s in blink.rs, one size up.
    struct FakeConn<'a> {
        response: &'a [u8],
        pos: usize,
        written: &'a RefCell<Vec<u8>>,
    }

    impl ErrorType for FakeConn<'_> {
        // embedded-io implements its `Error` trait for `Infallible`, so a
        // fake that can't fail needs no error type of its own.
        type Error = Infallible;
    }

    impl Read for FakeConn<'_> {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Infallible> {
            let n = (self.response.len() - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.response[self.pos..self.pos + n]);
            self.pos += n;
            // Once the canned bytes are drained this returns Ok(0), which
            // is embedded-io's contract for end-of-stream.
            Ok(n)
        }
    }

    impl Write for FakeConn<'_> {
        async fn write(&mut self, buf: &[u8]) -> Result<usize, Infallible> {
            self.written.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        // `flush` has no default impl, and reqwless does call it after
        // writing the request headers.
        async fn flush(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
    }

    struct FakeTcp<'a> {
        response: &'a [u8],
        written: &'a RefCell<Vec<u8>>,
        connected_to: &'a RefCell<Option<SocketAddr>>,
    }

    impl TcpConnect for FakeTcp<'_> {
        type Error = Infallible;
        // A generic associated type (GAT): each borrow of the client yields
        // a connection tied to that borrow's lifetime.
        type Connection<'c>
            = FakeConn<'c>
        where
            Self: 'c;

        async fn connect(&self, remote: SocketAddr) -> Result<FakeConn<'_>, Infallible> {
            *self.connected_to.borrow_mut() = Some(remote);
            Ok(FakeConn {
                response: self.response,
                pos: 0,
                written: self.written,
            })
        }
    }

    // -- Minimal executor ----------------------------------------------------

    /// Drive a future to completion by polling in a bounded loop.
    ///
    /// Unlike blink's test (which must observe `Pending` between clock
    /// steps), everything these fakes await is immediately ready — one poll
    /// would do. The bounded loop is cheap insurance: it fails loudly
    /// instead of spinning forever if a future ever parks for real.
    fn run<F: Future>(fut: F) -> F::Output {
        let mut fut = pin!(fut);
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..64 {
            if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
                return out;
            }
        }
        panic!(
            "future still pending after 64 polls — did a fake return Pending with a noop waker?"
        );
    }

    // -- Tests ---------------------------------------------------------------

    // Same URL the board uses. Hostname form matters: reqwless resolves
    // every request through `Dns` (even IP literals), so FakeDns is
    // genuinely on the path being tested.
    const URL: &str = "http://example.com/";

    #[test]
    fn get_status_returns_the_status_code() {
        let written = RefCell::new(Vec::new());
        let connected_to = RefCell::new(None);
        let tcp = FakeTcp {
            response: b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n",
            written: &written,
            connected_to: &connected_to,
        };
        let dns = FakeDns { fail: false };

        let status = run(get_status_with(&tcp, &dns, URL));
        assert_eq!(status.unwrap(), 200);

        // The client connected to FakeDns's answer, port 80 from `http://`.
        let expected = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 80);
        assert_eq!(*connected_to.borrow(), Some(expected));

        // What actually went over the "wire". Contains-style assertions so
        // a reqwless upgrade that adds a header doesn't break the test.
        let sent = String::from_utf8(written.borrow().clone()).unwrap();
        assert!(sent.starts_with("GET / HTTP/1.1\r\n"), "sent: {sent:?}");
        assert!(sent.contains("\r\nHost: example.com\r\n"), "sent: {sent:?}");
        assert!(sent.ends_with("\r\n\r\n"), "sent: {sent:?}");
    }

    #[test]
    fn get_status_reports_non_2xx_as_a_status_not_an_error() {
        let written = RefCell::new(Vec::new());
        let connected_to = RefCell::new(None);
        let tcp = FakeTcp {
            response: b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n",
            written: &written,
            connected_to: &connected_to,
        };
        let dns = FakeDns { fail: false };

        // An HTTP error status is still a successful HTTP exchange: the
        // function reports the code, it doesn't judge it.
        let status = run(get_status_with(&tcp, &dns, URL));
        assert_eq!(status.unwrap(), 404);
    }

    #[test]
    fn dns_failure_surfaces_as_http_error() {
        let written = RefCell::new(Vec::new());
        let connected_to = RefCell::new(None);
        let tcp = FakeTcp {
            response: b"", // never reached: resolution fails first
            written: &written,
            connected_to: &connected_to,
        };
        let dns = FakeDns { fail: true };

        // This exercises the `From<reqwless::Error>` impl through a real
        // `?` inside get_status_with, not just a direct conversion.
        let err = run(get_status_with(&tcp, &dns, URL)).unwrap_err();
        assert!(matches!(err, HttpError::Request(reqwless::Error::Dns)));
        assert!(
            connected_to.borrow().is_none(),
            "must not connect after DNS failure"
        );
    }

    #[test]
    fn reqwless_errors_convert_via_from() {
        let err: HttpError = reqwless::Error::Dns.into();
        assert!(matches!(err, HttpError::Request(reqwless::Error::Dns)));
    }
}
