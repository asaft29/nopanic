use anyhow::Result;
use async_trait::async_trait;
use common::{RelayStream, RelayTlsConfig, server_name_from_addr};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;

/// Abstraction over the transport layer used to connect to relay nodes.
/// Enables swapping TCP+TLS for WebSocket tunnels or test mocks without
/// changing the circuit-building logic.
#[async_trait]
pub trait TransportLayer: Send + Sync {
    async fn connect(&self, addr: SocketAddr, tls_fingerprint: &str) -> Result<RelayStream>;
}

/// The default TCP+TLS transport implementation used in production.
pub struct TcpTlsTransport;

#[async_trait]
impl TransportLayer for TcpTlsTransport {
    async fn connect(&self, addr: SocketAddr, tls_fingerprint: &str) -> Result<RelayStream> {
        let tcp_stream = TcpStream::connect(addr).await?;
        let connector = RelayTlsConfig::make_tls_connector(tls_fingerprint)?;
        let server_name = server_name_from_addr(addr);
        let tls_stream = connector.connect(server_name, tcp_stream).await?;
        Ok(Box::new(tls_stream))
    }
}

/// Raw TCP transport — no TLS wrapping.
/// Used in development mode when TLS is disabled via Cargo features.
pub struct RawTcpTransport;

#[async_trait]
impl TransportLayer for RawTcpTransport {
    async fn connect(&self, addr: SocketAddr, _fingerprint: &str) -> Result<RelayStream> {
        let tcp = TcpStream::connect(addr).await?;
        Ok(Box::new(tcp))
    }
}

/// Factory: returns a transport layer implementation.
/// With TLS enabled (default), returns a TLS-based transport.
/// Without TLS, returns a raw TCP transport.
#[cfg(feature = "tls")]
pub fn create_transport() -> Arc<dyn TransportLayer> {
    Arc::new(TcpTlsTransport)
}

#[cfg(not(feature = "tls"))]
pub fn create_transport() -> Arc<dyn TransportLayer> {
    Arc::new(RawTcpTransport)
}
