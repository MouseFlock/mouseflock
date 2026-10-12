//! SPAKE2 and the key-confirmation tags (design D4, D5).
//!
//! The tags bind the SPAKE2 key to the connection: the TLS exporter value and
//! both certificate keys. A relay in the middle sees two different
//! connections, so its tags never match.

use hkdf::Hkdf;
use model::PeerId;
use proto::pairing::{PAKE_LEN, TAG_LEN};
use rand_core::{CryptoRng, RngCore};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity as SpakeIdentity, Password, Spake2};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::Code;

/// The TLS exporter label `transport` uses for [`Binding::exporter`].
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-mouseflock-pair-v1";

/// Bytes `transport` exports for [`Binding::exporter`], with an empty context.
pub const EXPORTER_LEN: usize = 32;

/// SPAKE2 symmetric identity, and the HKDF salt for the tags.
const PROTOCOL_LABEL: &[u8] = b"mouseflock pair v1";

/// What ties a pairing to one connection, as this side sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The connection's TLS exporter value.
    pub exporter: [u8; EXPORTER_LEN],
    /// The key in the inviter's TLS certificate.
    pub inviter: PeerId,
    /// The key in the joiner's TLS certificate.
    pub joiner: PeerId,
}

/// One side's SPAKE2 run, between sending its message and receiving the
/// other's.
pub(crate) struct Pake {
    state: Spake2<Ed25519Group>,
}

impl Pake {
    /// Starts SPAKE2 for `code`, returning the state and the message to send.
    pub(crate) fn start<R: CryptoRng + RngCore>(
        code: &Code,
        rng: &mut R,
    ) -> (Self, [u8; PAKE_LEN]) {
        let (state, message) = Spake2::<Ed25519Group>::start_symmetric_with_rng(
            &Password::new(code.password()),
            &SpakeIdentity::new(PROTOCOL_LABEL),
            rng,
        );
        let mut out = [0; PAKE_LEN];
        // Symmetric mode always produces a side byte and a 32-byte point.
        out.copy_from_slice(&message);
        (Self { state }, out)
    }

    /// Finishes with the other side's message. `None` if it is not a valid
    /// symmetric-mode message.
    pub(crate) fn finish(self, theirs: &[u8; PAKE_LEN], binding: &Binding) -> Option<Tags> {
        let mut key = self.state.finish(theirs).ok()?;
        let tags = Tags::derive(&key, binding);
        key.zeroize();
        Some(tags)
    }
}

/// Both sides' expected key-confirmation tags.
pub(crate) struct Tags {
    pub(crate) joiner: [u8; TAG_LEN],
    pub(crate) inviter: [u8; TAG_LEN],
}

impl Drop for Tags {
    fn drop(&mut self) {
        self.joiner.zeroize();
        self.inviter.zeroize();
    }
}

impl Tags {
    fn derive(key: &[u8], binding: &Binding) -> Self {
        let hkdf = Hkdf::<Sha256>::new(Some(PROTOCOL_LABEL), key);
        let expand = |role: &[u8]| {
            let mut tag = [0; TAG_LEN];
            // 32 bytes is far below HKDF-SHA256's 8,160-byte limit.
            let _ = hkdf.expand_multi_info(
                &[
                    role,
                    &binding.exporter,
                    &binding.inviter.0,
                    &binding.joiner.0,
                ],
                &mut tag,
            );
            tag
        };
        Self {
            joiner: expand(b"joiner"),
            inviter: expand(b"inviter"),
        }
    }
}

/// Compares tags in constant time.
pub(crate) fn tags_match(received: &[u8; TAG_LEN], expected: &[u8; TAG_LEN]) -> bool {
    received.ct_eq(expected).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_rng;
    use std::net::Ipv4Addr;

    fn binding() -> Binding {
        Binding {
            exporter: [0xee; 32],
            inviter: PeerId([0xaa; 32]),
            joiner: PeerId([0xbb; 32]),
        }
    }

    /// A fixed code, 137-xxxxxxx, from a seeded generator.
    fn code() -> Code {
        Code::generate(Ipv4Addr::new(192, 168, 1, 137), 24, &mut test_rng(7))
    }

    fn run(code_j: &Code, code_i: &Code, bind_j: &Binding, bind_i: &Binding) -> (Tags, Tags) {
        let (joiner, msg_j) = Pake::start(code_j, &mut test_rng(10));
        let (inviter, msg_i) = Pake::start(code_i, &mut test_rng(20));
        (
            joiner.finish(&msg_i, bind_j).unwrap(),
            inviter.finish(&msg_j, bind_i).unwrap(),
        )
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    // Captured from this implementation with the seeds above. A change here
    // means the derivation, the SPAKE2 crate, or the code format changed, and
    // peers built before and after would no longer pair.
    const KNOWN_JOINER_TAG: &str =
        "b152bd20747458fea9391951c1d213a17de4d230829f1a1fa1a78b428459ad1f";
    const KNOWN_INVITER_TAG: &str =
        "7ec7876f0139662c7d0a7b63ecc9fa83d561557437c5c5f3722da5942a09d1ab";

    #[test]
    fn known_answer() {
        let c = code();
        let (_, message) = Pake::start(&c, &mut test_rng(10));
        assert_eq!(message[0], b'S');
        let (j, i) = run(&c, &c, &binding(), &binding());
        assert_eq!(hex(&j.joiner), KNOWN_JOINER_TAG);
        assert_eq!(hex(&j.inviter), KNOWN_INVITER_TAG);
        assert_eq!((j.joiner, j.inviter), (i.joiner, i.inviter));
    }

    #[test]
    fn joiner_and_inviter_tags_differ() {
        let (j, _) = run(&code(), &code(), &binding(), &binding());
        assert_ne!(j.joiner, j.inviter);
    }

    #[test]
    fn different_codes_disagree() {
        let other = code().with_new_secret(&mut test_rng(8));
        assert_ne!(other, code());
        let (j, i) = run(&code(), &other, &binding(), &binding());
        assert!(!tags_match(&j.joiner, &i.joiner));
        assert!(!tags_match(&i.inviter, &j.inviter));
    }

    #[test]
    fn every_binding_input_changes_both_tags() {
        let c = code();
        let (base, _) = run(&c, &c, &binding(), &binding());
        let mut variants = Vec::new();
        let mut b = binding();
        b.exporter[31] ^= 1;
        variants.push(b);
        let mut b = binding();
        b.inviter.0[0] ^= 1;
        variants.push(b);
        let mut b = binding();
        b.joiner.0[0] ^= 1;
        variants.push(b);
        for changed in variants {
            let (t, _) = run(&c, &c, &changed, &changed);
            assert_ne!(t.joiner, base.joiner);
            assert_ne!(t.inviter, base.inviter);
        }
    }

    #[test]
    fn corrupt_message_is_rejected() {
        let c = code();
        let (p, _) = Pake::start(&c, &mut test_rng(1));
        let mut bad_side = [0u8; PAKE_LEN];
        bad_side[0] = b'A';
        assert!(p.finish(&bad_side, &binding()).is_none());
    }
}
