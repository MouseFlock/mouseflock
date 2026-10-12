//! The machine's Ed25519 identity key and its file format.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use model::PeerId;
use rand_core::{CryptoRng, RngCore};
use zeroize::Zeroizing;

use crate::FileError;

/// Bytes in an identity file: magic, version, seed.
pub const IDENTITY_FILE_LEN: usize = 37;

const MAGIC: &[u8; 4] = b"MFID";
const VERSION: u8 = 1;

/// PKCS#8 v1 header for an Ed25519 private key (RFC 8410), followed by the
/// 32-byte seed.
const PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// This machine's identity key. The private half is wiped when dropped.
pub struct Identity {
    key: SigningKey,
}

impl Identity {
    /// A new key from `rng`. The service passes the OS generator.
    pub fn generate<R: CryptoRng + RngCore>(rng: &mut R) -> Self {
        Self {
            key: SigningKey::generate(rng),
        }
    }

    /// This machine's peer identity: the public key.
    pub fn peer_id(&self) -> PeerId {
        PeerId(self.key.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }

    /// The identity file: `MFID`, version 1, the 32-byte seed.
    pub fn to_file(&self) -> Zeroizing<[u8; IDENTITY_FILE_LEN]> {
        let mut out = Zeroizing::new([0; IDENTITY_FILE_LEN]);
        out[..4].copy_from_slice(MAGIC);
        out[4] = VERSION;
        out[5..].copy_from_slice(self.key.as_bytes());
        out
    }

    /// Reads an identity file. A malformed file is an error, never a new key:
    /// a new key would silently drop this machine from its desk.
    pub fn from_file(bytes: &[u8]) -> Result<Self, FileError> {
        let bytes: &[u8; IDENTITY_FILE_LEN] = bytes.try_into().map_err(|_| FileError::Length)?;
        let (magic, rest) = bytes.split_at(4);
        if magic != MAGIC {
            return Err(FileError::Magic);
        }
        let (version, seed) = rest.split_at(1);
        if version != [VERSION] {
            return Err(FileError::Version);
        }
        let mut secret = Zeroizing::new([0; 32]);
        secret.copy_from_slice(seed);
        Ok(Self {
            key: SigningKey::from_bytes(&secret),
        })
    }

    /// The private key as PKCS#8 DER, for the TLS certificate `transport`
    /// builds.
    pub fn to_pkcs8_der(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(48));
        out.extend_from_slice(&PKCS8_PREFIX);
        out.extend_from_slice(self.key.as_bytes());
        out
    }
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("peer_id", &self.peer_id())
            .finish_non_exhaustive()
    }
}

/// Whether `signature` is `signer`'s signature over `message`. Strict: weak
/// keys and non-canonical signatures fail.
pub fn verify(signer: &PeerId, message: &[u8], signature: &[u8; 64]) -> bool {
    VerifyingKey::from_bytes(&signer.0)
        .and_then(|key| key.verify_strict(message, &Signature::from_bytes(signature)))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_rng;

    #[test]
    fn first_start_creates_an_identity() {
        let identity = Identity::generate(&mut test_rng(1));
        let file = identity.to_file();
        assert_eq!(&file[..5], b"MFID\x01");
        let public = VerifyingKey::from_bytes(&identity.peer_id().0).unwrap();
        assert_eq!(public, identity.key.verifying_key());
    }

    #[test]
    fn identity_survives_a_restart() {
        let before = Identity::generate(&mut test_rng(2));
        let old_signature = before.sign(b"hello");
        let after = Identity::from_file(&*before.to_file()).unwrap();
        assert_eq!(after.peer_id(), before.peer_id());
        assert!(verify(&after.peer_id(), b"hello", &old_signature));
        assert!(verify(&before.peer_id(), b"again", &after.sign(b"again")));
    }

    #[test]
    fn corrupt_identity_file() {
        let good = *Identity::generate(&mut test_rng(3)).to_file();
        assert!(matches!(
            Identity::from_file(&good[..36]),
            Err(FileError::Length)
        ));
        let mut long = good.to_vec();
        long.push(0);
        assert!(matches!(Identity::from_file(&long), Err(FileError::Length)));
        let mut magic = good;
        magic[0] = b'X';
        assert!(matches!(Identity::from_file(&magic), Err(FileError::Magic)));
        let mut version = good;
        version[4] = 2;
        assert!(matches!(
            Identity::from_file(&version),
            Err(FileError::Version)
        ));
    }

    #[test]
    fn wrong_signer_or_message_fails() {
        let a = Identity::generate(&mut test_rng(4));
        let b = Identity::generate(&mut test_rng(5));
        let signature = a.sign(b"m");
        assert!(!verify(&b.peer_id(), b"m", &signature));
        assert!(!verify(&a.peer_id(), b"n", &signature));
    }

    #[test]
    fn pkcs8_matches_rfc_8410_example() {
        // RFC 8410, section 10.3.
        let seed: [u8; 32] = [
            0xd4, 0xee, 0x72, 0xdb, 0xf9, 0x13, 0x58, 0x4a, 0xd5, 0xb6, 0xd8, 0xf1, 0xf7, 0x69,
            0xf8, 0xad, 0x3a, 0xfe, 0x7c, 0x28, 0xcb, 0xf1, 0xd4, 0xfb, 0xe0, 0x97, 0xa8, 0x8f,
            0x44, 0x75, 0x58, 0x42,
        ];
        let mut file = [0; IDENTITY_FILE_LEN];
        file[..5].copy_from_slice(b"MFID\x01");
        file[5..].copy_from_slice(&seed);
        let der = Identity::from_file(&file).unwrap().to_pkcs8_der();
        let mut expected = PKCS8_PREFIX.to_vec();
        expected.extend_from_slice(&seed);
        assert_eq!(*der, expected);
        assert_eq!(&der[..2], &[0x30, 0x2e]);
    }
}
