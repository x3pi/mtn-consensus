// Copyright (c) MetaNode Team
// SPDX-License-Identifier: Apache-2.0

//! Socket abstraction layer - supports both Unix and TCP sockets.

use anyhow::Result;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpStream, UnixStream};

/// Socket address type - supports both Unix domain sockets and TCP sockets
#[derive(Debug, Clone)]
pub enum SocketAddress {
    /// Unix domain socket path (e.g., "/tmp/socket.sock")
    Unix(String),
    /// TCP socket address (e.g., "192.168.1.100:9001")
    Tcp(SocketAddr),
}

impl SocketAddress {
    /// Parse socket address from string with auto-detection
    ///
    /// Format:
    /// - Unix: "/tmp/socket.sock" or "unix:///tmp/socket.sock"
    /// - TCP: "tcp://host:port" or "host:port"
    ///
    /// # Examples
    /// ```
    /// let unix_addr = SocketAddress::parse("/tmp/socket.sock").unwrap();
    /// let tcp_addr = SocketAddress::parse("tcp://192.168.1.100:9001").unwrap();
    /// let tcp_addr2 = SocketAddress::parse("192.168.1.100:9001").unwrap();
    /// ```
    pub fn parse(addr: &str) -> Result<Self> {
        if addr.starts_with("tcp://") {
            // TCP format: "tcp://host:port"
            let addr_str = addr
                .strip_prefix("tcp://")
                .expect("prefix checked by starts_with");
            let sock_addr: SocketAddr = addr_str
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid TCP address '{}': {}", addr_str, e))?;
            Ok(SocketAddress::Tcp(sock_addr))
        } else if addr.starts_with("unix://") {
            // Unix format: "unix:///tmp/socket.sock"
            let path = addr
                .strip_prefix("unix://")
                .expect("prefix checked by starts_with");
            Ok(SocketAddress::Unix(path.to_string()))
        } else if addr.contains(':') && !addr.starts_with('/') {
            // TCP format without prefix: "host:port" or "192.168.1.100:9001"
            let sock_addr: SocketAddr = addr
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid TCP address '{}': {}", addr, e))?;
            Ok(SocketAddress::Tcp(sock_addr))
        } else {
            // Default to Unix socket (path format)
            Ok(SocketAddress::Unix(addr.to_string()))
        }
    }

    /// Get display string for logging
    pub fn as_str(&self) -> String {
        match self {
            SocketAddress::Unix(path) => path.clone(),
            SocketAddress::Tcp(addr) => format!("tcp://{}", addr),
        }
    }
}

/// Unified socket stream - wraps either Unix or TCP stream
pub enum SocketStream {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl SocketStream {
    /// Connect to a socket address with retry logic
    ///
    /// For TCP: includes timeout and TCP keepalive settings
    /// For Unix: connects directly to socket file
    pub async fn connect(addr: &SocketAddress, timeout_secs: u64) -> Result<Self> {
        match addr {
            SocketAddress::Unix(path) => {
                let stream = UnixStream::connect(path).await.map_err(|e| {
                    anyhow::anyhow!("Failed to connect to Unix socket '{}': {}", path, e)
                })?;

                // Tune UDS buffer sizes for high-throughput blocks
                let std_stream = stream
                    .into_std()
                    .map_err(|e| anyhow::anyhow!("Failed to convert UnixStream: {}", e))?;
                let socket = socket2::Socket::from(std_stream);
                let _ = socket.set_send_buffer_size(32 * 1024 * 1024);
                let _ = socket.set_recv_buffer_size(32 * 1024 * 1024);
                let stream = UnixStream::from_std(socket.into())
                    .map_err(|e| anyhow::anyhow!("Failed to restore UnixStream: {}", e))?;

                Ok(SocketStream::Unix(stream))
            }
            SocketAddress::Tcp(sock_addr) => {
                use tokio::time::{timeout, Duration};

                // Connect with timeout
                let stream = timeout(
                    Duration::from_secs(timeout_secs),
                    TcpStream::connect(sock_addr),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "TCP connection timeout after {}s to {}",
                        timeout_secs,
                        sock_addr
                    )
                })?
                .map_err(|e| {
                    anyhow::anyhow!("Failed to connect to TCP socket '{}': {}", sock_addr, e)
                })?;

                // T1-3: Configure TCP keepalive with specific intervals to detect dead connections
                // quickly. Default OS keepalive is ~2 hours — far too slow for consensus.
                // With these settings: detect dead connections within ~25 seconds.
                let socket = socket2::Socket::from(stream.into_std()?);
                let keepalive = socket2::TcpKeepalive::new()
                    .with_time(std::time::Duration::from_secs(10)) // Start probing after 10s idle
                    .with_interval(std::time::Duration::from_secs(5)) // Probe every 5s
                    .with_retries(3); // Give up after 3 failed probes
                socket
                    .set_tcp_keepalive(&keepalive)
                    .map_err(|e| anyhow::anyhow!("Failed to set TCP keepalive: {}", e))?;

                // Tune TCP buffer sizes
                let _ = socket.set_send_buffer_size(32 * 1024 * 1024);
                let _ = socket.set_recv_buffer_size(32 * 1024 * 1024);

                // Convert back to tokio TcpStream
                let stream = TcpStream::from_std(socket.into())?;

                Ok(SocketStream::Tcp(stream))
            }
        }
    }

    /// Check if stream is writable (for connection health check)
    pub async fn writable(&mut self) -> std::io::Result<()> {
        match self {
            SocketStream::Unix(s) => s.writable().await,
            SocketStream::Tcp(s) => s.writable().await,
        }
    }
}

// Implement AsyncRead for SocketStream
impl AsyncRead for SocketStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match &mut *self {
            SocketStream::Unix(s) => Pin::new(s).poll_read(cx, buf),
            SocketStream::Tcp(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

// Implement AsyncWrite for SocketStream
impl AsyncWrite for SocketStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            SocketStream::Unix(s) => Pin::new(s).poll_write(cx, buf),
            SocketStream::Tcp(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            SocketStream::Unix(s) => Pin::new(s).poll_flush(cx),
            SocketStream::Tcp(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            SocketStream::Unix(s) => Pin::new(s).poll_shutdown(cx),
            SocketStream::Tcp(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tcp_prefix() {
        let addr = SocketAddress::parse("tcp://127.0.0.1:9001").unwrap();
        match addr {
            SocketAddress::Tcp(sock_addr) => {
                assert_eq!(sock_addr.ip().to_string(), "127.0.0.1");
                assert_eq!(sock_addr.port(), 9001);
            }
            _ => panic!("Expected Tcp variant"),
        }
    }

    #[test]
    fn test_parse_unix_prefix() {
        let addr = SocketAddress::parse("unix:///tmp/socket.sock").unwrap();
        match addr {
            SocketAddress::Unix(path) => {
                assert_eq!(path, "/tmp/socket.sock");
            }
            _ => panic!("Expected Unix variant"),
        }
    }

    #[test]
    fn test_parse_tcp_bare() {
        let addr = SocketAddress::parse("192.168.1.100:9001").unwrap();
        match addr {
            SocketAddress::Tcp(sock_addr) => {
                assert_eq!(sock_addr.ip().to_string(), "192.168.1.100");
                assert_eq!(sock_addr.port(), 9001);
            }
            _ => panic!("Expected Tcp variant"),
        }
    }

    #[test]
    fn test_parse_unix_default() {
        // Path without any prefix defaults to Unix
        let addr = SocketAddress::parse("/tmp/go-executor.sock").unwrap();
        match addr {
            SocketAddress::Unix(path) => {
                assert_eq!(path, "/tmp/go-executor.sock");
            }
            _ => panic!("Expected Unix variant"),
        }
    }

    #[test]
    fn test_parse_tcp_invalid() {
        let result = SocketAddress::parse("tcp://not-an-address");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Invalid TCP address"),
            "Unexpected error: {}",
            err_msg
        );
    }
}
