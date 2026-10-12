//! Pairing and desk-membership messages (ADR 0004, Amendment 2).
//!
//! `PairPake` and `PairConfirm` run the pairing exchange; `MemberRecord` and
//! `RecordsDone` carry signed membership records between members. This module
//! only checks layouts. Signing, verifying, and deriving keys belong to the
//! `pairing` crate.

use crate::codec::fixed_payload;
use crate::{EncodeError, ProtocolError, frame, registry};

/// Bytes in a SPAKE2 symmetric-mode message: a side byte and a point.
pub const PAKE_LEN: usize = 33;

/// Bytes in a key-confirmation tag.
pub const TAG_LEN: usize = 32;

/// Bytes in a peer identity (an Ed25519 public key).
pub const KEY_LEN: usize = 32;

/// Bytes in an Ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;

/// Longest machine name, in bytes of UTF-8.
pub const MAX_NAME_LEN: usize = 63;

/// Prefix of the bytes a membership record's signature covers.
pub const SIGNATURE_CONTEXT: &[u8] = b"mouseflock member record v1";

/// `MemberRecord` bytes before the name: kind, subject, epoch, signer, name
/// length.
const RECORD_HEAD: usize = 1 + KEY_LEN + 8 + KEY_LEN + 1;

/// Payload length of a removal record, which has no name.
pub const REMOVAL_LEN: usize = RECORD_HEAD + SIGNATURE_LEN;

/// Shortest `PairConfirm` payload: a tag, a name length, one byte of name.
const MIN_CONFIRM_LEN: usize = TAG_LEN + 2;

/// Longest `PairConfirm` payload.
const MAX_CONFIRM_LEN: usize = TAG_LEN + 1 + MAX_NAME_LEN;

/// Longest `MemberRecord` payload.
const MAX_RECORD_LEN: usize = REMOVAL_LEN + MAX_NAME_LEN;

const KIND_ADD: u8 = 1;
const KIND_REMOVE: u8 = 2;

/// A machine name: 1 to 63 bytes of UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Name<'a>(&'a str);

impl<'a> Name<'a> {
    /// `name` if its length is in range.
    pub fn new(name: &'a str) -> Option<Self> {
        (1..=MAX_NAME_LEN)
            .contains(&name.len())
            .then_some(Self(name))
    }

    pub const fn as_str(self) -> &'a str {
        self.0
    }

    fn decode(bytes: &'a [u8]) -> Result<Self, ProtocolError> {
        let name = core::str::from_utf8(bytes).map_err(|_| ProtocolError::NameNotUtf8)?;
        Self::new(name).ok_or(ProtocolError::InvalidNameLength { len: bytes.len() })
    }

    fn len_byte(self) -> u8 {
        // `new` caps the length at 63.
        u8::try_from(self.0.len()).unwrap_or(u8::MAX)
    }
}

/// `PairPake`: one side's SPAKE2 message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairPake(pub [u8; PAKE_LEN]);

impl PairPake {
    pub fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        fixed_payload(registry::PAIR_PAKE, payload).map(Self)
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(registry::PAIR_PAKE, &self.0, out)
    }
}

/// `PairConfirm`: a key-confirmation tag and the sender's machine name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairConfirm<'a> {
    pub tag: [u8; TAG_LEN],
    pub name: Name<'a>,
}

impl<'a> PairConfirm<'a> {
    pub fn decode(payload: &'a [u8]) -> Result<Self, ProtocolError> {
        let ty = registry::PAIR_CONFIRM;
        let mut c = Cursor::new(ty, payload);
        let tag = c.array::<TAG_LEN>(MIN_CONFIRM_LEN)?;
        let [n] = c.array::<1>(MIN_CONFIRM_LEN)?;
        let n = usize::from(n);
        if n == 0 || n > MAX_NAME_LEN {
            return Err(ProtocolError::InvalidNameLength { len: n });
        }
        c.expect_total(TAG_LEN, 1, n)?;
        let name = Name::decode(c.bytes(n)?)?;
        Ok(Self { tag, name })
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        let mut payload = Vec::with_capacity(MAX_CONFIRM_LEN);
        payload.extend_from_slice(&self.tag);
        payload.push(self.name.len_byte());
        payload.extend_from_slice(self.name.as_str().as_bytes());
        frame::encode(registry::PAIR_CONFIRM, &payload, out)
    }
}

/// What a membership record does to its subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change<'a> {
    /// Adds the subject, under this machine name.
    Add(Name<'a>),
    Remove,
}

/// `MemberRecord`: one signed membership record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberRecord<'a> {
    pub subject: [u8; KEY_LEN],
    pub epoch: u64,
    pub signer: [u8; KEY_LEN],
    pub change: Change<'a>,
    pub signature: [u8; SIGNATURE_LEN],
}

impl<'a> MemberRecord<'a> {
    pub fn decode(payload: &'a [u8]) -> Result<Self, ProtocolError> {
        let ty = registry::MEMBER_RECORD;
        let mut c = Cursor::new(ty, payload);
        let [kind] = c.array::<1>(REMOVAL_LEN)?;
        let subject = c.array::<KEY_LEN>(REMOVAL_LEN)?;
        let epoch = u64::from_le_bytes(c.array::<8>(REMOVAL_LEN)?);
        let signer = c.array::<KEY_LEN>(REMOVAL_LEN)?;
        let [n] = c.array::<1>(REMOVAL_LEN)?;
        let n = usize::from(n);
        let name_ok = match kind {
            KIND_ADD => (1..=MAX_NAME_LEN).contains(&n),
            KIND_REMOVE => n == 0,
            _ => return Err(ProtocolError::InvalidRecordKind { kind }),
        };
        if !name_ok {
            return Err(ProtocolError::InvalidNameLength { len: n });
        }
        c.expect_total(REMOVAL_LEN, 0, n)?;
        let name = c.bytes(n)?;
        let signature = c.array::<SIGNATURE_LEN>(REMOVAL_LEN)?;
        let change = if kind == KIND_ADD {
            Change::Add(Name::decode(name)?)
        } else {
            Change::Remove
        };
        Ok(Self {
            subject,
            epoch,
            signer,
            change,
            signature,
        })
    }

    /// Appends the record without its signature.
    fn encode_unsigned(&self, out: &mut Vec<u8>) {
        let (kind, name) = match self.change {
            Change::Add(name) => (KIND_ADD, Some(name)),
            Change::Remove => (KIND_REMOVE, None),
        };
        out.push(kind);
        out.extend_from_slice(&self.subject);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&self.signer);
        out.push(name.map_or(0, Name::len_byte));
        if let Some(name) = name {
            out.extend_from_slice(name.as_str().as_bytes());
        }
    }

    /// The bytes the signature covers: [`SIGNATURE_CONTEXT`], then the
    /// record's encoding without the signature.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SIGNATURE_CONTEXT.len().saturating_add(REMOVAL_LEN));
        out.extend_from_slice(SIGNATURE_CONTEXT);
        self.encode_unsigned(&mut out);
        out
    }

    /// The `MemberRecord` payload.
    pub fn payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAX_RECORD_LEN);
        self.encode_unsigned(&mut out);
        out.extend_from_slice(&self.signature);
        out
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(registry::MEMBER_RECORD, &self.payload(), out)
    }
}

/// `RecordsDone`: the end of one side's records in an exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordsDone;

impl RecordsDone {
    pub fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        fixed_payload::<0>(registry::RECORDS_DONE, payload).map(|_| Self)
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(registry::RECORDS_DONE, &[], out)
    }
}

/// Any pairing or membership message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingMessage<'a> {
    Pake(PairPake),
    Confirm(PairConfirm<'a>),
    Record(MemberRecord<'a>),
    RecordsDone,
}

impl<'a> PairingMessage<'a> {
    /// Decodes a message of type `ty`. Other types are
    /// [`ProtocolError::UnknownType`].
    pub fn decode(ty: u16, payload: &'a [u8]) -> Result<Self, ProtocolError> {
        match ty {
            registry::PAIR_PAKE => PairPake::decode(payload).map(Self::Pake),
            registry::PAIR_CONFIRM => PairConfirm::decode(payload).map(Self::Confirm),
            registry::MEMBER_RECORD => MemberRecord::decode(payload).map(Self::Record),
            registry::RECORDS_DONE => RecordsDone::decode(payload).map(|_| Self::RecordsDone),
            _ => Err(ProtocolError::UnknownType { ty }),
        }
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        match self {
            Self::Pake(m) => m.encode_frame(out),
            Self::Confirm(m) => m.encode_frame(out),
            Self::Record(m) => m.encode_frame(out),
            Self::RecordsDone => RecordsDone.encode_frame(out),
        }
    }
}

/// Reads fields front to back; every shortfall is a payload-length error.
struct Cursor<'a> {
    ty: u16,
    payload: &'a [u8],
    rest: &'a [u8],
}

impl<'a> Cursor<'a> {
    const fn new(ty: u16, payload: &'a [u8]) -> Self {
        Self {
            ty,
            payload,
            rest: payload,
        }
    }

    fn length_error(&self, expected: usize) -> ProtocolError {
        ProtocolError::PayloadLength {
            ty: self.ty,
            expected,
            actual: self.payload.len(),
        }
    }

    /// The next `N` bytes; `min_len` is the shortest valid payload, for the
    /// error.
    fn array<const N: usize>(&mut self, min_len: usize) -> Result<[u8; N], ProtocolError> {
        let (head, rest) = self
            .rest
            .split_first_chunk::<N>()
            .ok_or_else(|| self.length_error(min_len))?;
        self.rest = rest;
        Ok(*head)
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], ProtocolError> {
        let (head, rest) = self
            .rest
            .split_at_checked(n)
            .ok_or_else(|| self.length_error(n))?;
        self.rest = rest;
        Ok(head)
    }

    /// Checks the whole payload is exactly `a + b + c` bytes.
    fn expect_total(&self, a: usize, b: usize, c: usize) -> Result<(), ProtocolError> {
        let expected = a
            .checked_add(b)
            .and_then(|ab| ab.checked_add(c))
            .unwrap_or(usize::MAX);
        if self.payload.len() == expected {
            Ok(())
        } else {
            Err(self.length_error(expected))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> Name<'_> {
        Name::new(s).unwrap()
    }

    fn record(change: Change<'_>) -> MemberRecord<'_> {
        MemberRecord {
            subject: [1; 32],
            epoch: 0x0102_0304_0506_0708,
            signer: [2; 32],
            change,
            signature: [3; 64],
        }
    }

    fn frame_payload(bytes: &[u8]) -> (u16, &[u8]) {
        match frame::decode(bytes).unwrap() {
            frame::Decoded::Frame { frame, .. } => (frame.ty, frame.payload),
            frame::Decoded::NeedMore => panic!("incomplete frame"),
        }
    }

    #[test]
    fn pake_round_trips() {
        let pake = PairPake([7; PAKE_LEN]);
        let mut out = Vec::new();
        pake.encode_frame(&mut out).unwrap();
        let (ty, payload) = frame_payload(&out);
        assert_eq!(ty, registry::PAIR_PAKE);
        assert_eq!(PairPake::decode(payload), Ok(pake));
    }

    #[test]
    fn pake_of_wrong_length_is_rejected() {
        for len in [32, 34] {
            assert_eq!(
                PairPake::decode(&vec![0; len]),
                Err(ProtocolError::PayloadLength {
                    ty: registry::PAIR_PAKE,
                    expected: 33,
                    actual: len
                })
            );
        }
    }

    #[test]
    fn confirm_round_trips() {
        let confirm = PairConfirm {
            tag: [9; 32],
            name: name("DESK-B"),
        };
        let mut out = Vec::new();
        confirm.encode_frame(&mut out).unwrap();
        let (ty, payload) = frame_payload(&out);
        assert_eq!(ty, registry::PAIR_CONFIRM);
        assert_eq!(payload.len(), 32 + 1 + 6);
        assert_eq!(PairConfirm::decode(payload), Ok(confirm));
    }

    #[test]
    fn confirm_with_empty_name_is_rejected() {
        let mut payload = vec![0; 32];
        payload.push(0);
        assert_eq!(
            PairConfirm::decode(&payload),
            Err(ProtocolError::InvalidNameLength { len: 0 })
        );
    }

    #[test]
    fn confirm_with_64_byte_name_is_rejected() {
        let mut payload = vec![0; 32];
        payload.push(64);
        payload.extend([b'a'; 64]);
        assert_eq!(
            PairConfirm::decode(&payload),
            Err(ProtocolError::InvalidNameLength { len: 64 })
        );
    }

    #[test]
    fn confirm_with_trailing_byte_is_rejected() {
        let mut payload = vec![0; 32];
        payload.extend([1, b'a', b'b']);
        assert_eq!(
            PairConfirm::decode(&payload),
            Err(ProtocolError::PayloadLength {
                ty: registry::PAIR_CONFIRM,
                expected: 34,
                actual: 35
            })
        );
    }

    #[test]
    fn invalid_utf8_name_is_rejected() {
        let mut payload = vec![0; 32];
        payload.extend([2, 0xc3, 0x28]);
        assert_eq!(
            PairConfirm::decode(&payload),
            Err(ProtocolError::NameNotUtf8)
        );
    }

    #[test]
    fn records_round_trip() {
        for change in [Change::Add(name("Living room PC")), Change::Remove] {
            let r = record(change);
            let mut out = Vec::new();
            r.encode_frame(&mut out).unwrap();
            let (ty, payload) = frame_payload(&out);
            assert_eq!(ty, registry::MEMBER_RECORD);
            assert_eq!(MemberRecord::decode(payload), Ok(r));
        }
    }

    #[test]
    fn removal_record_layout() {
        let payload = record(Change::Remove).payload();
        assert_eq!(payload.len(), 138);
        assert_eq!(payload[0], 2);
        assert_eq!(&payload[33..41], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(payload[73], 0);
        assert_eq!(&payload[74..], &[3; 64]);
    }

    #[test]
    fn signed_bytes_are_context_then_record_without_signature() {
        let r = record(Change::Add(name("A")));
        let signed = r.signed_bytes();
        let payload = r.payload();
        assert_eq!(&signed[..SIGNATURE_CONTEXT.len()], SIGNATURE_CONTEXT);
        assert_eq!(
            &signed[SIGNATURE_CONTEXT.len()..],
            &payload[..payload.len() - 64]
        );
    }

    #[test]
    fn unknown_record_kind_is_rejected() {
        let mut payload = record(Change::Remove).payload();
        payload[0] = 3;
        assert_eq!(
            MemberRecord::decode(&payload),
            Err(ProtocolError::InvalidRecordKind { kind: 3 })
        );
    }

    #[test]
    fn addition_with_no_name_is_rejected() {
        let mut payload = record(Change::Remove).payload();
        payload[0] = 1;
        assert_eq!(
            MemberRecord::decode(&payload),
            Err(ProtocolError::InvalidNameLength { len: 0 })
        );
    }

    #[test]
    fn removal_with_a_name_is_rejected() {
        let mut payload = record(Change::Add(name("A"))).payload();
        payload[0] = 2;
        assert_eq!(
            MemberRecord::decode(&payload),
            Err(ProtocolError::InvalidNameLength { len: 1 })
        );
    }

    #[test]
    fn truncated_record_is_rejected() {
        let payload = record(Change::Add(name("AB"))).payload();
        assert_eq!(
            MemberRecord::decode(&payload[..payload.len() - 1]),
            Err(ProtocolError::PayloadLength {
                ty: registry::MEMBER_RECORD,
                expected: 140,
                actual: 139
            })
        );
    }

    #[test]
    fn records_done_with_a_payload_is_rejected() {
        assert_eq!(RecordsDone::decode(&[]), Ok(RecordsDone));
        assert_eq!(
            RecordsDone::decode(&[0]),
            Err(ProtocolError::PayloadLength {
                ty: registry::RECORDS_DONE,
                expected: 0,
                actual: 1
            })
        );
    }

    #[test]
    fn name_bounds() {
        assert_eq!(Name::new(""), None);
        assert!(Name::new(&"a".repeat(63)).is_some());
        assert_eq!(Name::new(&"a".repeat(64)), None);
    }

    #[test]
    fn decode_by_type() {
        assert_eq!(
            PairingMessage::decode(registry::RECORDS_DONE, &[]),
            Ok(PairingMessage::RecordsDone)
        );
        assert_eq!(
            PairingMessage::decode(registry::HELLO, &[1, 0, 1, 0]),
            Err(ProtocolError::UnknownType {
                ty: registry::HELLO
            })
        );
    }
}
