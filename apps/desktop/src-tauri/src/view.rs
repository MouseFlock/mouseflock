//! What React is shown (design D3): the service's status and notices as
//! plain serde data, and the tray tooltip. Mapping only; nothing here
//! decides anything about input or pairing.

use ipc::ui::{
    FocusOf, JoinFailedReason, Member as IpcMember, NotOpenedReason, Pairing as IpcPairing,
    PairingClosedReason, PeerKey, Status, UiEvent,
};
use model::Side;
use serde::Serialize;

/// Hex digits in a short key.
const SHORT_KEY: usize = 8;
/// Longest tooltip Windows shows, in UTF-16 units.
const TOOLTIP_MAX: usize = 127;

/// The whole window state. `status` is `None` while not connected.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct View {
    pub connected: bool,
    pub status: Option<StatusView>,
}

impl View {
    pub const fn disconnected() -> Self {
        Self {
            connected: false,
            status: None,
        }
    }

    pub fn connected(status: &Status) -> Self {
        Self {
            connected: true,
            status: Some(StatusView::from(status)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    pub key: String,
    pub name: String,
    pub agent_linked: bool,
    pub members: Vec<MemberView>,
    pub pairing: PairingView,
    pub focus: FocusView,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MemberView {
    pub key: String,
    /// Shown beside the name only when another member has the same name.
    pub short: Option<String>,
    pub name: String,
    pub up: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum PairingView {
    Closed,
    #[serde(rename_all = "camelCase")]
    Open {
        code: String,
        left_ms: u32,
    },
}

/// Where this PC's input is. `name` is the member's display name.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum FocusView {
    Local,
    On { key: String, name: String },
    ControlledBy { key: String, name: String },
}

/// Something that just happened, for the window or the Undo popup.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Notice {
    Paired {
        key: String,
        name: String,
    },
    JoinFailed {
        reason: JoinFailedTag,
    },
    PairingClosed {
        reason: PairingClosedTag,
    },
    PairingNotOpened {
        reason: NotOpenedTag,
    },
    #[serde(rename_all = "camelCase")]
    Learned {
        key: String,
        name: String,
        side: SideTag,
        left_ms: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JoinFailedTag {
    WrongCode,
    NotReady,
    ProtocolError,
    DeskFull,
    ConnectionLost,
    Unreachable,
    MistypedCode,
    MalformedCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PairingClosedTag {
    Joined,
    Expired,
    ByUser,
    TooManyFailures,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NotOpenedTag {
    NoNetwork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SideTag {
    Left,
    Right,
    Top,
    Bottom,
}

impl From<JoinFailedReason> for JoinFailedTag {
    fn from(r: JoinFailedReason) -> Self {
        match r {
            JoinFailedReason::WrongCode => Self::WrongCode,
            JoinFailedReason::NotReady => Self::NotReady,
            JoinFailedReason::ProtocolError => Self::ProtocolError,
            JoinFailedReason::DeskFull => Self::DeskFull,
            JoinFailedReason::ConnectionLost => Self::ConnectionLost,
            JoinFailedReason::Unreachable => Self::Unreachable,
            JoinFailedReason::MistypedCode => Self::MistypedCode,
            JoinFailedReason::MalformedCode => Self::MalformedCode,
        }
    }
}

impl From<PairingClosedReason> for PairingClosedTag {
    fn from(r: PairingClosedReason) -> Self {
        match r {
            PairingClosedReason::Joined => Self::Joined,
            PairingClosedReason::Expired => Self::Expired,
            PairingClosedReason::ByUser => Self::ByUser,
            PairingClosedReason::TooManyFailures => Self::TooManyFailures,
        }
    }
}

impl From<NotOpenedReason> for NotOpenedTag {
    fn from(r: NotOpenedReason) -> Self {
        match r {
            NotOpenedReason::NoNetwork => Self::NoNetwork,
        }
    }
}

impl From<Side> for SideTag {
    fn from(s: Side) -> Self {
        match s {
            Side::Left => Self::Left,
            Side::Right => Self::Right,
            Side::Top => Self::Top,
            Side::Bottom => Self::Bottom,
        }
    }
}

impl From<&Status> for StatusView {
    fn from(s: &Status) -> Self {
        Self {
            key: hex(&s.key),
            name: s.name.clone(),
            agent_linked: s.agent_linked,
            members: members(&s.members),
            pairing: match &s.pairing {
                IpcPairing::Closed => PairingView::Closed,
                IpcPairing::Open { code, left_ms } => PairingView::Open {
                    code: code.clone(),
                    left_ms: *left_ms,
                },
            },
            focus: match s.focus {
                FocusOf::Local => FocusView::Local,
                FocusOf::On(k) => FocusView::On {
                    key: hex(&k),
                    name: name_of(Some(s), &k),
                },
                FocusOf::ControlledBy(k) => FocusView::ControlledBy {
                    key: hex(&k),
                    name: name_of(Some(s), &k),
                },
            },
        }
    }
}

fn members(list: &[IpcMember]) -> Vec<MemberView> {
    list.iter()
        .map(|m| {
            let clash = list.iter().filter(|o| o.name == m.name).count() > 1;
            MemberView {
                key: hex(&m.key),
                short: clash.then(|| short(&m.key)),
                name: m.name.clone(),
                up: m.up,
            }
        })
        .collect()
}

/// A member's name from the latest status, or its short key when the status
/// does not list it.
pub fn name_of(status: Option<&Status>, key: &PeerKey) -> String {
    status
        .and_then(|s| s.members.iter().find(|m| &m.key == key))
        .map_or_else(|| short(key), |m| m.name.clone())
}

/// A notice for React, or `None` for a `Status`. Names come from `latest`.
pub fn notice(event: &UiEvent, latest: Option<&Status>) -> Option<Notice> {
    Some(match event {
        UiEvent::Status(_) => return None,
        UiEvent::Paired { key, name } => Notice::Paired {
            key: hex(key),
            name: name.clone(),
        },
        UiEvent::JoinFailed(r) => Notice::JoinFailed {
            reason: (*r).into(),
        },
        UiEvent::PairingClosed(r) => Notice::PairingClosed {
            reason: (*r).into(),
        },
        UiEvent::PairingNotOpened(r) => Notice::PairingNotOpened {
            reason: (*r).into(),
        },
        UiEvent::Learned { key, side, undo_ms } => Notice::Learned {
            key: hex(key),
            name: name_of(latest, key),
            side: (*side).into(),
            left_ms: *undo_ms,
        },
    })
}

/// The tray tooltip for `view`, at most 127 UTF-16 units.
pub fn tooltip(view: &View) -> String {
    let text = match &view.status {
        None => "MouseFlock: service isn't running".to_owned(),
        Some(s) => match &s.focus {
            FocusView::On { name, .. } => format!("MouseFlock: controlling {name}"),
            FocusView::ControlledBy { name, .. } => format!("MouseFlock: controlled by {name}"),
            FocusView::Local if s.members.is_empty() => "MouseFlock: no other PCs yet".to_owned(),
            FocusView::Local => {
                let up = s.members.iter().filter(|m| m.up).count();
                let all = s.members.len();
                let pcs = if all == 1 { "PC" } else { "PCs" };
                format!("MouseFlock: {up} of {all} {pcs} connected")
            }
        },
    };
    cut(&text, TOOLTIP_MAX)
}

/// `text` cut to `max` UTF-16 units, ending in "…" when cut.
fn cut(text: &str, max: usize) -> String {
    if text.encode_utf16().count() <= max {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        if units + c.len_utf16() > max - 1 {
            break;
        }
        units += c.len_utf16();
        out.push(c);
    }
    out.push('…');
    out
}

pub fn hex(key: &PeerKey) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

fn short(key: &PeerKey) -> String {
    hex(key)[..SHORT_KEY].to_owned()
}

/// A key from 64 hex digits, as React sends it back.
pub fn parse_key(text: &str) -> Option<PeerKey> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let hi = char::from(pair[0]).to_digit(16)?;
        let lo = char::from(pair[1]).to_digit(16)?;
        key[i] = u8::try_from(hi * 16 + lo).ok()?;
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(n: u8) -> PeerKey {
        [n; 32]
    }

    fn member(n: u8, name: &str, up: bool) -> IpcMember {
        IpcMember {
            key: key(n),
            name: name.into(),
            up,
        }
    }

    fn status(members: Vec<IpcMember>, focus: FocusOf) -> Status {
        Status {
            key: key(1),
            name: "DESK".into(),
            agent_linked: true,
            pairing: IpcPairing::Open {
                code: "137-4829153".into(),
                left_ms: 600_000,
            },
            focus,
            members,
        }
    }

    #[test]
    fn status_maps_to_json() {
        let s = status(vec![member(2, "OFFICE", true)], FocusOf::On(key(2)));
        let json = serde_json::to_value(View::connected(&s)).unwrap();
        assert_eq!(
            json,
            json!({
                "connected": true,
                "status": {
                    "key": "01".repeat(32),
                    "name": "DESK",
                    "agentLinked": true,
                    "members": [{ "key": "02".repeat(32), "short": null, "name": "OFFICE", "up": true }],
                    "pairing": { "state": "open", "code": "137-4829153", "leftMs": 600_000 },
                    "focus": { "state": "on", "key": "02".repeat(32), "name": "OFFICE" },
                }
            })
        );
        assert_eq!(
            serde_json::to_value(View::disconnected()).unwrap(),
            json!({ "connected": false, "status": null })
        );
    }

    #[test]
    fn short_keys_only_on_name_clashes() {
        let s = status(
            vec![
                member(2, "OFFICE", true),
                member(3, "OFFICE", false),
                member(4, "LAPTOP", true),
            ],
            FocusOf::Local,
        );
        let v = StatusView::from(&s);
        assert_eq!(v.members[0].short.as_deref(), Some("02020202"));
        assert_eq!(v.members[1].short.as_deref(), Some("03030303"));
        assert_eq!(v.members[2].short, None);
    }

    #[test]
    fn every_reason_has_its_tag() {
        let joins = [
            (JoinFailedReason::WrongCode, "wrongCode"),
            (JoinFailedReason::NotReady, "notReady"),
            (JoinFailedReason::ProtocolError, "protocolError"),
            (JoinFailedReason::DeskFull, "deskFull"),
            (JoinFailedReason::ConnectionLost, "connectionLost"),
            (JoinFailedReason::Unreachable, "unreachable"),
            (JoinFailedReason::MistypedCode, "mistypedCode"),
            (JoinFailedReason::MalformedCode, "malformedCode"),
        ];
        for (r, tag) in joins {
            let n = notice(&UiEvent::JoinFailed(r), None).unwrap();
            assert_eq!(
                serde_json::to_value(n).unwrap(),
                json!({ "kind": "joinFailed", "reason": tag })
            );
        }
        let closes = [
            (PairingClosedReason::Joined, "joined"),
            (PairingClosedReason::Expired, "expired"),
            (PairingClosedReason::ByUser, "byUser"),
            (PairingClosedReason::TooManyFailures, "tooManyFailures"),
        ];
        for (r, tag) in closes {
            let n = notice(&UiEvent::PairingClosed(r), None).unwrap();
            assert_eq!(
                serde_json::to_value(n).unwrap(),
                json!({ "kind": "pairingClosed", "reason": tag })
            );
        }
        let n = notice(&UiEvent::PairingNotOpened(NotOpenedReason::NoNetwork), None).unwrap();
        assert_eq!(
            serde_json::to_value(n).unwrap(),
            json!({ "kind": "pairingNotOpened", "reason": "noNetwork" })
        );
        let n = notice(
            &UiEvent::Paired {
                key: key(2),
                name: "OFFICE".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(n).unwrap(),
            json!({ "kind": "paired", "key": "02".repeat(32), "name": "OFFICE" })
        );
        assert_eq!(
            notice(&UiEvent::Status(status(vec![], FocusOf::Local)), None),
            None
        );
    }

    #[test]
    fn learned_names_the_member_or_its_short_key() {
        let s = status(vec![member(2, "OFFICE", true)], FocusOf::Local);
        let sides = [
            (Side::Left, "left"),
            (Side::Right, "right"),
            (Side::Top, "top"),
            (Side::Bottom, "bottom"),
        ];
        for (side, tag) in sides {
            let event = UiEvent::Learned {
                key: key(2),
                side,
                undo_ms: 10_000,
            };
            assert_eq!(
                serde_json::to_value(notice(&event, Some(&s)).unwrap()).unwrap(),
                json!({ "kind": "learned", "key": "02".repeat(32), "name": "OFFICE", "side": tag, "leftMs": 10_000 })
            );
        }
        let unknown = UiEvent::Learned {
            key: key(9),
            side: Side::Left,
            undo_ms: 10_000,
        };
        let Some(Notice::Learned { name, .. }) = notice(&unknown, Some(&s)) else {
            panic!("not learned");
        };
        assert_eq!(name, "09090909");
    }

    #[test]
    fn tooltips() {
        assert_eq!(
            tooltip(&View::disconnected()),
            "MouseFlock: service isn't running"
        );
        let none = status(vec![], FocusOf::Local);
        assert_eq!(
            tooltip(&View::connected(&none)),
            "MouseFlock: no other PCs yet"
        );
        let some = status(
            vec![member(2, "OFFICE", true), member(3, "LAPTOP", false)],
            FocusOf::Local,
        );
        assert_eq!(
            tooltip(&View::connected(&some)),
            "MouseFlock: 1 of 2 PCs connected"
        );
        let one = status(vec![member(2, "OFFICE", true)], FocusOf::Local);
        assert_eq!(
            tooltip(&View::connected(&one)),
            "MouseFlock: 1 of 1 PC connected"
        );
        let on = status(vec![member(2, "OFFICE", true)], FocusOf::On(key(2)));
        assert_eq!(
            tooltip(&View::connected(&on)),
            "MouseFlock: controlling OFFICE"
        );
        let by = status(
            vec![member(2, "OFFICE", true)],
            FocusOf::ControlledBy(key(2)),
        );
        assert_eq!(
            tooltip(&View::connected(&by)),
            "MouseFlock: controlled by OFFICE"
        );
    }

    #[test]
    fn long_tooltips_are_cut() {
        let long = "É".repeat(63);
        let mut s = status(vec![member(2, &long, true)], FocusOf::On(key(2)));
        s.members[0].name = format!("{long}{long}");
        let t = tooltip(&View::connected(&s));
        assert_eq!(t.encode_utf16().count(), TOOLTIP_MAX);
        assert!(t.ends_with('…'));
        let short = status(vec![member(2, &long, true)], FocusOf::On(key(2)));
        assert!(!tooltip(&View::connected(&short)).ends_with('…'));
    }

    #[test]
    fn keys_round_trip_through_hex() {
        let k: PeerKey = std::array::from_fn(|i| u8::try_from(i * 7).unwrap());
        assert_eq!(parse_key(&hex(&k)), Some(k));
        assert_eq!(parse_key(&hex(&k).to_uppercase()), Some(k));
        assert_eq!(parse_key(&hex(&k)[..63]), None);
        assert_eq!(parse_key(&format!("{}zz", &hex(&k)[..62])), None);
        assert_eq!(parse_key(""), None);
    }
}
