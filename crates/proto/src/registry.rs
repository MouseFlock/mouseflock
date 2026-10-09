//! Message types, the channel each may use, and the version that introduced
//! it. `docs/protocol.md` lists the same table for implementers.

use crate::ProtocolError;

/// Never a valid message type.
pub const RESERVED: u16 = 0x0000;

/// Version handshake. The first frame on the control stream.
pub const HELLO: u16 = 0x0001;

/// A SPAKE2 message during pairing.
pub const PAIR_PAKE: u16 = 0x0002;

/// A key-confirmation tag and machine name during pairing.
pub const PAIR_CONFIRM: u16 = 0x0003;

/// One signed desk-membership record.
pub const MEMBER_RECORD: u16 = 0x0004;

/// The end of one side's records in an exchange.
pub const RECORDS_DONE: u16 = 0x0005;

/// Focus moves to the receiver through one of its sides.
pub const FOCUS_ENTER: u16 = 0x0006;

/// The receiver is controlled by another peer; the sender keeps focus.
pub const FOCUS_REFUSED: u16 = 0x0007;

/// The cursor left the sender's edge toward a target peer.
pub const EDGE_EXIT: u16 = 0x0008;

/// The sender's own user took focus back.
pub const TAKEN_BACK: u16 = 0x0009;

/// The sender no longer forwards input to the receiver.
pub const FOCUS_WITHDRAWN: u16 = 0x000a;

/// Remove the receiver's slots on a side that map to the sender.
pub const FORGET_SIDE: u16 = 0x000b;

/// A forwarded key press or release, with the motion mark.
pub const KEY_INPUT: u16 = 0x000c;

/// A forwarded button press or release, with the motion mark.
pub const BUTTON_INPUT: u16 = 0x000d;

/// Forwarded wheel movement, with the motion mark.
pub const WHEEL_INPUT: u16 = 0x000e;

/// Show the secure attention screen.
pub const SECURE_ATTENTION: u16 = 0x000f;

/// Forwarded motion as running totals. Datagram only.
pub const MOTION: u16 = 0x0010;

/// A new shareable copy on the sender, by ID; no text.
pub const CLIPBOARD_OFFER: u16 = 0x0011;

/// Asks the receiver for its copy with this ID.
pub const CLIPBOARD_REQUEST: u16 = 0x0012;

/// Starts a clipboard stream: the copy's ID and the text's length.
pub const CLIPBOARD_DATA: u16 = 0x0013;

/// Bytes of the copy on a clipboard stream.
pub const CLIPBOARD_CHUNK: u16 = 0x0014;

/// Starts an image clipboard stream: the copy's ID and the PNG's length.
/// Version 2.
pub const CLIPBOARD_IMAGE_DATA: u16 = 0x0015;

/// Starts a clipboard stream carrying a file copy's manifest: the copy's ID
/// and the manifest's length. Version 3.
pub const CLIPBOARD_FILES_DATA: u16 = 0x0016;

/// Asks the receiver for one file of its file copy: the copy's ID and the
/// file's index in the manifest. Version 3.
pub const FILE_CONTENT_REQUEST: u16 = 0x0017;

/// Starts a file content stream: the copy's ID, the file's index in the
/// manifest, and its length as a u64. Version 3.
pub const FILE_CONTENT_DATA: u16 = 0x0018;

/// Where a message type may be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// The control stream.
    Stream,
    Datagram,
    /// A unidirectional stream carrying one clipboard transfer: a copy's
    /// text or image, a file copy's manifest, or one file's contents.
    Transfer,
}

/// One registered message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeInfo {
    pub ty: u16,
    pub name: &'static str,
    pub channel: Channel,
    /// Protocol version that introduced this type.
    pub since: u16,
}

/// Every message type, in type order.
pub const REGISTRY: &[TypeInfo] = &[
    stream(HELLO, "Hello"),
    stream(PAIR_PAKE, "PairPake"),
    stream(PAIR_CONFIRM, "PairConfirm"),
    stream(MEMBER_RECORD, "MemberRecord"),
    stream(RECORDS_DONE, "RecordsDone"),
    stream(FOCUS_ENTER, "FocusEnter"),
    stream(FOCUS_REFUSED, "FocusRefused"),
    stream(EDGE_EXIT, "EdgeExit"),
    stream(TAKEN_BACK, "TakenBack"),
    stream(FOCUS_WITHDRAWN, "FocusWithdrawn"),
    stream(FORGET_SIDE, "ForgetSide"),
    stream(KEY_INPUT, "KeyInput"),
    stream(BUTTON_INPUT, "ButtonInput"),
    stream(WHEEL_INPUT, "WheelInput"),
    stream(SECURE_ATTENTION, "SecureAttention"),
    datagram(MOTION, "Motion"),
    stream(CLIPBOARD_OFFER, "ClipboardOffer"),
    stream(CLIPBOARD_REQUEST, "ClipboardRequest"),
    transfer(CLIPBOARD_DATA, "ClipboardData"),
    transfer(CLIPBOARD_CHUNK, "ClipboardChunk"),
    TypeInfo {
        ty: CLIPBOARD_IMAGE_DATA,
        name: "ClipboardImageData",
        channel: Channel::Transfer,
        since: 2,
    },
    TypeInfo {
        ty: CLIPBOARD_FILES_DATA,
        name: "ClipboardFilesData",
        channel: Channel::Transfer,
        since: 3,
    },
    TypeInfo {
        ty: FILE_CONTENT_REQUEST,
        name: "FileContentRequest",
        channel: Channel::Stream,
        since: 3,
    },
    TypeInfo {
        ty: FILE_CONTENT_DATA,
        name: "FileContentData",
        channel: Channel::Transfer,
        since: 3,
    },
];

/// A stream message type introduced in version 1.
const fn stream(ty: u16, name: &'static str) -> TypeInfo {
    TypeInfo {
        ty,
        name,
        channel: Channel::Stream,
        since: 1,
    }
}

/// A datagram message type introduced in version 1.
const fn datagram(ty: u16, name: &'static str) -> TypeInfo {
    TypeInfo {
        ty,
        name,
        channel: Channel::Datagram,
        since: 1,
    }
}

/// A clipboard-stream message type introduced in version 1.
const fn transfer(ty: u16, name: &'static str) -> TypeInfo {
    TypeInfo {
        ty,
        name,
        channel: Channel::Transfer,
        since: 1,
    }
}

/// The message type `ty` as defined in protocol `version`.
pub fn lookup(ty: u16, version: u16) -> Option<&'static TypeInfo> {
    REGISTRY
        .iter()
        .find(|info| info.ty == ty && info.since <= version)
}

/// Checks that `ty` is defined in `version` and allowed on `channel`.
pub fn check(ty: u16, channel: Channel, version: u16) -> Result<&'static TypeInfo, ProtocolError> {
    if ty == RESERVED {
        return Err(ProtocolError::ReservedType);
    }
    let info = lookup(ty, version).ok_or(ProtocolError::UnknownType { ty })?;
    if info.channel != channel {
        return Err(ProtocolError::WrongChannel { ty });
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_1_defines_exactly_these_types() {
        use Channel::{Datagram, Stream, Transfer};
        let names: Vec<_> = REGISTRY
            .iter()
            .filter(|i| i.since == 1)
            .map(|i| (i.ty, i.name, i.channel))
            .collect();
        assert_eq!(
            names,
            [
                (0x0001, "Hello", Stream),
                (0x0002, "PairPake", Stream),
                (0x0003, "PairConfirm", Stream),
                (0x0004, "MemberRecord", Stream),
                (0x0005, "RecordsDone", Stream),
                (0x0006, "FocusEnter", Stream),
                (0x0007, "FocusRefused", Stream),
                (0x0008, "EdgeExit", Stream),
                (0x0009, "TakenBack", Stream),
                (0x000a, "FocusWithdrawn", Stream),
                (0x000b, "ForgetSide", Stream),
                (0x000c, "KeyInput", Stream),
                (0x000d, "ButtonInput", Stream),
                (0x000e, "WheelInput", Stream),
                (0x000f, "SecureAttention", Stream),
                (0x0010, "Motion", Datagram),
                (0x0011, "ClipboardOffer", Stream),
                (0x0012, "ClipboardRequest", Stream),
                (0x0013, "ClipboardData", Transfer),
                (0x0014, "ClipboardChunk", Transfer),
            ]
        );
        for info in REGISTRY.iter().filter(|i| i.since == 1) {
            assert_eq!(check(info.ty, info.channel, 1), Ok(info));
        }
        assert_eq!(
            check(0x0015, Channel::Stream, 1),
            Err(ProtocolError::UnknownType { ty: 0x0015 })
        );
    }

    #[test]
    fn version_2_adds_only_clipboard_image_data() {
        let added: Vec<_> = REGISTRY
            .iter()
            .filter(|i| i.since == 2)
            .map(|i| (i.ty, i.name, i.channel))
            .collect();
        assert_eq!(added, [(0x0015, "ClipboardImageData", Channel::Transfer)]);
        assert!(REGISTRY.iter().all(|i| i.since <= 3));
        let info = check(CLIPBOARD_IMAGE_DATA, Channel::Transfer, 2).unwrap();
        assert_eq!(info.name, "ClipboardImageData");
        assert_eq!(
            check(CLIPBOARD_IMAGE_DATA, Channel::Transfer, 1),
            Err(ProtocolError::UnknownType {
                ty: CLIPBOARD_IMAGE_DATA
            })
        );
        assert_eq!(
            check(CLIPBOARD_IMAGE_DATA, Channel::Stream, 2),
            Err(ProtocolError::WrongChannel {
                ty: CLIPBOARD_IMAGE_DATA
            })
        );
        // Every version 1 type is still defined, unchanged, in version 2.
        for info in REGISTRY.iter().filter(|i| i.since == 1) {
            assert_eq!(check(info.ty, info.channel, 2), Ok(info));
        }
    }

    #[test]
    fn version_3_adds_only_the_file_types() {
        use Channel::{Stream, Transfer};
        let added: Vec<_> = REGISTRY
            .iter()
            .filter(|i| i.since == 3)
            .map(|i| (i.ty, i.name, i.channel))
            .collect();
        assert_eq!(
            added,
            [
                (0x0016, "ClipboardFilesData", Transfer),
                (0x0017, "FileContentRequest", Stream),
                (0x0018, "FileContentData", Transfer),
            ]
        );
        for info in REGISTRY.iter().filter(|i| i.since == 3) {
            assert_eq!(check(info.ty, info.channel, 3), Ok(info));
            assert_eq!(
                check(info.ty, info.channel, 2),
                Err(ProtocolError::UnknownType { ty: info.ty })
            );
        }
        assert_eq!(
            check(FILE_CONTENT_REQUEST, Transfer, 3),
            Err(ProtocolError::WrongChannel {
                ty: FILE_CONTENT_REQUEST
            })
        );
        for ty in [CLIPBOARD_FILES_DATA, FILE_CONTENT_DATA] {
            assert_eq!(
                check(ty, Stream, 3),
                Err(ProtocolError::WrongChannel { ty })
            );
        }
        // Every earlier type is still defined, unchanged, in version 3.
        for info in REGISTRY.iter().filter(|i| i.since < 3) {
            assert_eq!(check(info.ty, info.channel, 3), Ok(info));
        }
    }

    #[test]
    fn motion_on_a_stream_is_rejected() {
        assert_eq!(
            check(MOTION, Channel::Stream, 1),
            Err(ProtocolError::WrongChannel { ty: MOTION })
        );
    }

    #[test]
    fn key_input_in_a_datagram_is_rejected() {
        assert_eq!(
            check(KEY_INPUT, Channel::Datagram, 1),
            Err(ProtocolError::WrongChannel { ty: KEY_INPUT })
        );
    }

    #[test]
    fn pairing_message_in_a_datagram_is_rejected() {
        assert_eq!(
            check(MEMBER_RECORD, Channel::Datagram, 1),
            Err(ProtocolError::WrongChannel { ty: MEMBER_RECORD })
        );
    }

    #[test]
    fn reserved_type_is_rejected() {
        assert_eq!(
            check(RESERVED, Channel::Stream, 1),
            Err(ProtocolError::ReservedType)
        );
    }

    #[test]
    fn unknown_type_is_rejected() {
        assert_eq!(
            check(0x0042, Channel::Stream, 1),
            Err(ProtocolError::UnknownType { ty: 0x0042 })
        );
    }

    #[test]
    fn stream_type_in_datagram_is_rejected() {
        assert_eq!(
            check(HELLO, Channel::Datagram, 1),
            Err(ProtocolError::WrongChannel { ty: HELLO })
        );
    }

    #[test]
    fn clipboard_types_keep_to_their_streams() {
        for ty in [CLIPBOARD_OFFER, CLIPBOARD_REQUEST] {
            assert!(check(ty, Channel::Stream, 1).is_ok());
            assert_eq!(
                check(ty, Channel::Transfer, 1),
                Err(ProtocolError::WrongChannel { ty })
            );
        }
        for ty in [CLIPBOARD_DATA, CLIPBOARD_CHUNK] {
            assert!(check(ty, Channel::Transfer, 1).is_ok());
            assert_eq!(
                check(ty, Channel::Stream, 1),
                Err(ProtocolError::WrongChannel { ty })
            );
        }
        assert_eq!(
            check(KEY_INPUT, Channel::Transfer, 1),
            Err(ProtocolError::WrongChannel { ty: KEY_INPUT })
        );
    }

    #[test]
    fn registry_is_sorted_and_unique() {
        assert!(REGISTRY.windows(2).all(|w| w[0].ty < w[1].ty));
        assert!(REGISTRY.iter().all(|info| info.ty != RESERVED));
    }
}
