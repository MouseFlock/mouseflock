//! TLS identity: the self-signed certificate carrying this machine's Ed25519
//! key, reading a peer's key from its certificate, and the verifiers that
//! pin keys (ADR 0004, Amendment 2; design D2, D3).
//!
//! No CA and no names: a certificate is only a container for the key, and the
//! TLS 1.3 handshake proves the peer holds it.

use std::sync::Arc;

use model::PeerId;
use pairing::Identity;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error, PeerMisbehaved,
    SignatureScheme,
};

/// ALPN of a member connection.
pub const ALPN_MEMBER: &[u8] = b"mouseflock/1";

/// ALPN of a pairing connection.
pub const ALPN_PAIR: &[u8] = b"mouseflock-pair/1";

/// The server name every dialer sends. Verifiers ignore it.
pub const SERVER_NAME: &str = "mouseflock";

/// DER of an Ed25519 SubjectPublicKeyInfo up to the key (RFC 8410).
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Why a certificate could not give a peer identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// Not a parseable X.509 certificate.
    Malformed,
    /// A key other than a 32-byte Ed25519 key.
    NotEd25519,
}

/// The peer identity in `cert`: its Ed25519 public key. Every other field is
/// ignored.
pub fn peer_id_from_cert(cert: &CertificateDer<'_>) -> Result<PeerId, KeyError> {
    let parsed = webpki::EndEntityCert::try_from(cert).map_err(|_| KeyError::Malformed)?;
    let spki = parsed.subject_public_key_info();
    let key = spki
        .as_ref()
        .strip_prefix(&ED25519_SPKI_PREFIX)
        .ok_or(KeyError::NotEd25519)?;
    let key: [u8; 32] = key.try_into().map_err(|_| KeyError::NotEd25519)?;
    Ok(PeerId(key))
}

/// This machine's certificate and private key, made fresh at every start.
pub struct Credentials {
    pub cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}

impl Credentials {
    pub fn new(identity: &Identity) -> Result<Self, rcgen::Error> {
        let pkcs8 = identity.to_pkcs8_der();
        let key = PrivatePkcs8KeyDer::from(pkcs8.to_vec());
        let key_pair = rcgen::KeyPair::try_from(&key)?;
        let params = rcgen::CertificateParams::new(vec![SERVER_NAME.to_owned()])?;
        let cert = params.self_signed(&key_pair)?;
        Ok(Self {
            cert: cert.der().clone(),
            key,
        })
    }

    fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }
}

/// The crypto provider: ring, TLS 1.3.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// What a dialer accepts from the listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Only this member's key.
    Peer(PeerId),
    /// Any key: a pairing dial. The key becomes the inviter's identity.
    Any,
}

fn check_tls13(
    provider: &CryptoProvider,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, Error> {
    if dss.scheme != SignatureScheme::ED25519 {
        return Err(Error::PeerMisbehaved(
            PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme,
        ));
    }
    verify_tls13_signature(
        message,
        cert,
        dss,
        &provider.signature_verification_algorithms,
    )
}

fn key_of(cert: &CertificateDer<'_>) -> Result<PeerId, Error> {
    peer_id_from_cert(cert).map_err(|e| {
        Error::InvalidCertificate(match e {
            KeyError::Malformed => CertificateError::BadEncoding,
            KeyError::NotEd25519 => CertificateError::UnknownIssuer,
        })
    })
}

/// The dialer's check of the listener's certificate.
#[derive(Debug)]
pub struct ServerVerifier {
    expect: Expect,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for ServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let key = key_of(end_entity)?;
        match self.expect {
            Expect::Peer(expected) if expected != key => Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            )),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls13RequiredForQuic,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        check_tls13(&self.provider, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

/// The listener's check of the dialer's certificate: any Ed25519 key, decided
/// after the handshake.
#[derive(Debug)]
pub struct ClientVerifier {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for ClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        key_of(end_entity).map(|_| ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls13RequiredForQuic,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        check_tls13(&self.provider, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }
}

/// The listener's TLS config: both ALPNs, a client certificate required.
pub fn server_config(credentials: &Credentials) -> Result<rustls::ServerConfig, Error> {
    let provider = provider();
    let mut config = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(ClientVerifier { provider }))
        .with_single_cert(vec![credentials.cert.clone()], credentials.key())?;
    config.alpn_protocols = vec![ALPN_MEMBER.to_vec(), ALPN_PAIR.to_vec()];
    Ok(config)
}

/// A dialer's TLS config for one dial.
pub fn client_config(
    credentials: &Credentials,
    expect: Expect,
    alpn: &[u8],
) -> Result<rustls::ClientConfig, Error> {
    let provider = provider();
    let mut config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ServerVerifier { expect, provider }))
        .with_client_auth_cert(vec![credentials.cert.clone()], credentials.key())?;
    config.alpn_protocols = vec![alpn.to_vec()];
    Ok(config)
}

/// The peer identity in a QUIC connection's peer certificate.
pub fn connection_peer(connection: &quinn::Connection) -> Option<PeerId> {
    let identity = connection.peer_identity()?;
    let certs = identity.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    peer_id_from_cert(certs.first()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::OsRng;

    #[test]
    fn identity_from_the_certificate() {
        let identity = Identity::generate(&mut OsRng);
        let credentials = Credentials::new(&identity).unwrap();
        assert_eq!(peer_id_from_cert(&credentials.cert), Ok(identity.peer_id()));
    }

    #[test]
    fn certificates_differ_but_keys_match_across_starts() {
        let identity = Identity::generate(&mut OsRng);
        let a = Credentials::new(&identity).unwrap();
        let b = Credentials::new(&identity).unwrap();
        assert_eq!(peer_id_from_cert(&a.cert), peer_id_from_cert(&b.cert));
    }

    #[test]
    fn non_ed25519_certificate_is_rejected() {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let cert = rcgen::CertificateParams::new(vec!["x".to_owned()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        assert_eq!(peer_id_from_cert(cert.der()), Err(KeyError::NotEd25519));
    }

    #[test]
    fn malformed_certificate_is_rejected_without_panicking() {
        let identity = Identity::generate(&mut OsRng);
        let good = Credentials::new(&identity).unwrap().cert.to_vec();
        for len in 0..good.len() {
            let cut = CertificateDer::from(good[..len].to_vec());
            assert!(peer_id_from_cert(&cut).is_err());
        }
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..2_000 {
            let mut bytes = good.clone();
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let i = (state % bytes.len() as u64) as usize;
            bytes[i] ^= (state >> 32) as u8 | 1;
            let _ = peer_id_from_cert(&CertificateDer::from(bytes));
        }
    }
}
