//! QUIC endpoints: one UDP socket that both listens and dials, with the
//! liveness settings from design D5.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};

use crate::tls::{self, Credentials, Expect};

/// The port every machine listens on (UDP).
pub const PORT: u16 = 47391;

/// Send a keepalive after this long without sending anything else.
pub const KEEPALIVE: Duration = Duration::from_millis(250);

/// A peer silent for this long is lost.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(1);

/// Why an endpoint could not start.
#[derive(Debug)]
pub enum StartError {
    /// The UDP port could not be bound.
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
    /// The certificate or TLS configuration could not be built.
    Tls(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind { addr, source } => {
                write!(
                    f,
                    "cannot listen on UDP port {} ({addr}): {source}",
                    addr.port()
                )
            }
            Self::Tls(e) => write!(f, "cannot build TLS configuration: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config.keep_alive_interval(Some(KEEPALIVE));
    // 1 s fits in a VarInt of milliseconds.
    config.max_idle_timeout(quinn::IdleTimeout::try_from(IDLE_TIMEOUT).ok());
    // The peer may open the control stream and one clipboard stream at a
    // time; flow control holds the next transfer until the first ends.
    config.max_concurrent_bidi_streams(1u8.into());
    config.max_concurrent_uni_streams(1u8.into());
    Arc::new(config)
}

/// An endpoint bound to `addr` that accepts both kinds of connection.
pub fn bind(credentials: &Credentials, addr: SocketAddr) -> Result<quinn::Endpoint, StartError> {
    let tls = tls::server_config(credentials).map_err(|e| StartError::Tls(e.to_string()))?;
    let crypto = QuicServerConfig::try_from(tls).map_err(|e| StartError::Tls(e.to_string()))?;
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    server.transport_config(transport_config());
    quinn::Endpoint::server(server, addr).map_err(|source| StartError::Bind { addr, source })
}

/// The client config for one dial.
pub fn client(
    credentials: &Credentials,
    expect: Expect,
    alpn: &[u8],
) -> Result<quinn::ClientConfig, rustls::Error> {
    let tls = tls::client_config(credentials, expect, alpn)?;
    let crypto =
        QuicClientConfig::try_from(tls).map_err(|e| rustls::Error::General(e.to_string()))?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(transport_config());
    Ok(config)
}
