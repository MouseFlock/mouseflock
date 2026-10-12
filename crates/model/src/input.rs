//! Input events as the agent reports them, and input actions to inject.

/// Where a local input event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A keyboard or mouse attached to this machine.
    Physical,
    /// Produced by software: MouseFlock on a peer's behalf, or anything else.
    Injected,
}

/// A key, as a USB HID usage on the keyboard page (0x07).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(pub u16);

impl Key {
    pub const END: Key = Key(0x4d);
    pub const SCROLL_LOCK: Key = Key(0x47);
    pub const LEFT_CTRL: Key = Key(0xe0);
    pub const LEFT_SHIFT: Key = Key(0xe1);
    pub const LEFT_ALT: Key = Key(0xe2);
    pub const RIGHT_CTRL: Key = Key(0xe4);
    pub const RIGHT_SHIFT: Key = Key(0xe5);
    pub const RIGHT_ALT: Key = Key(0xe6);

    pub const fn is_ctrl(self) -> bool {
        self.0 == Self::LEFT_CTRL.0 || self.0 == Self::RIGHT_CTRL.0
    }

    pub const fn is_alt(self) -> bool {
        self.0 == Self::LEFT_ALT.0 || self.0 == Self::RIGHT_ALT.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// One local input event, as the agent's hooks and Raw Input report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Key {
        key: Key,
        down: bool,
    },
    Button {
        button: Button,
        down: bool,
    },
    /// Wheel movement in 1/120 of a notch, as Windows reports it: positive
    /// `dy` is away from the user, positive `dx` is to the right.
    Wheel {
        dx: i32,
        dy: i32,
    },
    /// Raw device movement, before pointer speed, and the cursor position in
    /// virtual-screen pixels after the OS applied it.
    Motion {
        dx: i32,
        dy: i32,
        cursor: crate::Point,
    },
}

/// Input to forward to a peer or inject into this machine's OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    Key {
        key: Key,
        down: bool,
    },
    Button {
        button: Button,
        down: bool,
    },
    /// Wheel movement in 1/120 of a notch; signs as for [`InputEvent::Wheel`].
    Wheel {
        dx: i32,
        dy: i32,
    },
    /// Raw relative movement; the machine injecting it applies its own
    /// pointer speed.
    Motion {
        dx: i32,
        dy: i32,
    },
}

/// Which local physical input the agent's hook keeps from this machine's OS.
/// Injected input always reaches the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// Local focus: withhold nothing.
    PassAll,
    /// Forwarding: withhold all physical input; it goes to the peer.
    WithholdAll,
    /// Controlled: withhold physical mouse input; keys pass through.
    WithholdMouse,
}
