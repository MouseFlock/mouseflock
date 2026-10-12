//! What the hooks and the Raw Input handler decide, as pure functions.

use model::{Button, CaptureMode, InputEvent, Origin};

/// `dwExtraInfo` on every input MouseFlock injects ("MFLK"). Raw Input carries
/// it in `RAWMOUSE::ulExtraInformation`.
pub(crate) const TAG: u32 = 0x4d46_4c4b;

const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_MOUSEWHEEL: u32 = 0x020a;
const WM_XBUTTONDOWN: u32 = 0x020b;
const WM_XBUTTONUP: u32 = 0x020c;
const WM_MOUSEHWHEEL: u32 = 0x020e;
const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;
/// `RAWMOUSE::usFlags` bit for absolute positions.
const MOUSE_MOVE_ABSOLUTE: u16 = 1;

/// The kind of event a hook sees, for withholding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A key on the HID keyboard page.
    Key,
    /// A key with no keyboard-page usage.
    OtherKey,
    /// `E0 2A` / `E0 36`.
    FakeShift,
    /// Any mouse event: move, button or wheel.
    Mouse,
}

/// True if the hook keeps this event from the OS (design D3).
pub(crate) fn withhold(mode: CaptureMode, kind: Kind, origin: Origin) -> bool {
    if origin == Origin::Injected {
        return false;
    }
    match kind {
        Kind::OtherKey => false,
        Kind::Key | Kind::FakeShift => mode == CaptureMode::WithholdAll,
        Kind::Mouse => matches!(mode, CaptureMode::WithholdAll | CaptureMode::WithholdMouse),
    }
}

/// A low-level mouse hook message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseMessage {
    /// Withheld or passed, never reported: motion comes from Raw Input.
    Move,
    /// A button or wheel event to report.
    Event(InputEvent),
    /// Anything else: passed and not reported.
    Other,
}

/// Decodes a low-level mouse hook message and its `mouseData`.
pub(crate) fn mouse_message(msg: u32, mouse_data: u32) -> MouseMessage {
    let button = |button, down| MouseMessage::Event(InputEvent::Button { button, down });
    let high = mouse_data >> 16;
    let delta = i32::from(high as u16 as i16);
    match msg {
        WM_MOUSEMOVE => MouseMessage::Move,
        WM_LBUTTONDOWN => button(Button::Left, true),
        WM_LBUTTONUP => button(Button::Left, false),
        WM_RBUTTONDOWN => button(Button::Right, true),
        WM_RBUTTONUP => button(Button::Right, false),
        WM_MBUTTONDOWN => button(Button::Middle, true),
        WM_MBUTTONUP => button(Button::Middle, false),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let down = msg == WM_XBUTTONDOWN;
            match high {
                XBUTTON1 => button(Button::X1, down),
                XBUTTON2 => button(Button::X2, down),
                _ => MouseMessage::Other,
            }
        }
        WM_MOUSEWHEEL => MouseMessage::Event(InputEvent::Wheel { dx: 0, dy: delta }),
        WM_MOUSEHWHEEL => MouseMessage::Event(InputEvent::Wheel { dx: delta, dy: 0 }),
        _ => MouseMessage::Other,
    }
}

/// The relative movement in a Raw Input mouse report, or `None` for an
/// absolute or zero report (design D4).
pub(crate) fn raw_motion(us_flags: u16, dx: i32, dy: i32) -> Option<(i32, i32)> {
    let absolute = us_flags & MOUSE_MOVE_ABSOLUTE != 0;
    (!absolute && (dx, dy) != (0, 0)).then_some((dx, dy))
}

/// The origin of a Raw Input move (design D4): MouseFlock's tag means
/// injected, a device handle means physical, and a report with neither
/// follows the last move the hook saw that MouseFlock did not inject.
pub(crate) fn motion_origin(extra: u32, has_device: bool, last_hook_move_injected: bool) -> Origin {
    if extra == TAG {
        Origin::Injected
    } else if has_device || !last_hook_move_injected {
        Origin::Physical
    } else {
        Origin::Injected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CaptureMode::{PassAll, WithholdAll, WithholdMouse};
    use Origin::{Injected, Physical};

    #[test]
    fn withhold_table() {
        // (kind, PassAll, WithholdAll, WithholdMouse) for physical input.
        let rows = [
            (Kind::Key, false, true, false),
            (Kind::Mouse, false, true, true),
            (Kind::OtherKey, false, false, false),
            (Kind::FakeShift, false, true, false),
        ];
        for (kind, pass, all, mouse) in rows {
            assert_eq!(withhold(PassAll, kind, Physical), pass, "{kind:?} PassAll");
            assert_eq!(
                withhold(WithholdAll, kind, Physical),
                all,
                "{kind:?} WithholdAll"
            );
            assert_eq!(
                withhold(WithholdMouse, kind, Physical),
                mouse,
                "{kind:?} WithholdMouse"
            );
            for mode in [PassAll, WithholdAll, WithholdMouse] {
                assert!(
                    !withhold(mode, kind, Injected),
                    "{kind:?} {mode:?} injected"
                );
            }
        }
    }

    fn event(msg: u32, data: u32) -> InputEvent {
        match mouse_message(msg, data) {
            MouseMessage::Event(e) => e,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn buttons() {
        let b = |button, down| InputEvent::Button { button, down };
        assert_eq!(event(WM_LBUTTONDOWN, 0), b(Button::Left, true));
        assert_eq!(event(WM_LBUTTONUP, 0), b(Button::Left, false));
        assert_eq!(event(WM_RBUTTONDOWN, 0), b(Button::Right, true));
        assert_eq!(event(WM_RBUTTONUP, 0), b(Button::Right, false));
        assert_eq!(event(WM_MBUTTONDOWN, 0), b(Button::Middle, true));
        assert_eq!(event(WM_MBUTTONUP, 0), b(Button::Middle, false));
        assert_eq!(event(WM_XBUTTONDOWN, 1 << 16), b(Button::X1, true));
        assert_eq!(event(WM_XBUTTONUP, 2 << 16), b(Button::X2, false));
        assert_eq!(mouse_message(WM_XBUTTONDOWN, 3 << 16), MouseMessage::Other);
    }

    #[test]
    fn wheel() {
        let up = 120u32 << 16;
        let down = u32::from(-120i16 as u16) << 16;
        assert_eq!(
            event(WM_MOUSEWHEEL, up),
            InputEvent::Wheel { dx: 0, dy: 120 }
        );
        assert_eq!(
            event(WM_MOUSEWHEEL, down),
            InputEvent::Wheel { dx: 0, dy: -120 }
        );
        assert_eq!(
            event(WM_MOUSEHWHEEL, up),
            InputEvent::Wheel { dx: 120, dy: 0 }
        );
        assert_eq!(
            event(WM_MOUSEHWHEEL, down),
            InputEvent::Wheel { dx: -120, dy: 0 }
        );
    }

    #[test]
    fn moves_and_unknown_messages() {
        assert_eq!(mouse_message(WM_MOUSEMOVE, 0), MouseMessage::Move);
        assert_eq!(mouse_message(0x0203, 0), MouseMessage::Other);
    }

    #[test]
    fn raw_reports() {
        assert_eq!(raw_motion(0, 7, -2), Some((7, -2)));
        assert_eq!(raw_motion(0, 0, 0), None);
        assert_eq!(raw_motion(MOUSE_MOVE_ABSOLUTE, 30000, 20000), None);
    }

    #[test]
    fn motion_origins() {
        // MouseFlock's tag wins whatever else is true.
        assert_eq!(motion_origin(TAG, true, false), Injected);
        assert_eq!(motion_origin(TAG, false, false), Injected);
        // A device handle means physical.
        assert_eq!(motion_origin(0, true, true), Physical);
        // Neither: follow the hook.
        assert_eq!(motion_origin(0, false, false), Physical);
        assert_eq!(motion_origin(0, false, true), Injected);
        assert_eq!(motion_origin(0x1234, false, true), Injected);
    }
}
