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

/// Perform a GET request and return the HTTP status code.
pub async fn get_status(stack: Stack<'static>, url: &str) -> Result<u16, HttpError> {
    // `<1, ...>` = one pooled TCP connection; this client makes one request
    // at a time.
    let state: TcpClientState<1, TCP_TX_SZ, TCP_RX_SZ> = TcpClientState::new();
    let tcp = TcpClient::new(stack, &state);
    let dns = DnsSocket::new(stack);
    let mut client = HttpClient::new(&tcp, &dns);

    let mut rx_buf = [0u8; HEADER_BUF];
    let mut req = client.request(Method::GET, url).await?;
    let resp = req.send(&mut rx_buf).await?;
    Ok(resp.status.0)
}

#[cfg(test)]
mod tests {
    use super::HttpError;

    // `get_status` itself needs a live embassy-net `Stack` (a real network
    // driver behind it), so it isn't unit-testable today — see ADR 0005 for
    // the future direction. What IS pure logic is the error conversion that
    // every `?` in get_status relies on.
    #[test]
    fn reqwless_errors_convert_via_from() {
        let err: HttpError = reqwless::Error::Dns.into();
        assert!(matches!(err, HttpError::Request(reqwless::Error::Dns)));
    }
}
