//! Pure state machine: focus owner, edge transitions, routing, layout learning.
//!
//! Depends only on `model`, `proto`, and `policy`. Never on `win32-*`,
//! `transport`, or `ipc`. Must build and test on Linux. Tests are replay tests:
//! a sequence of `model` events in, routing decisions out.
//!
//! # How it is driven
//!
//! Every machine runs one [`Engine`]. The host feeds it [`Event`]s: local input
//! from the agent (with a monotonic timestamp and its [`Origin`]), messages
//! from peers' engines, peers connecting and disconnecting, the agent going
//! away ([`Event::LocalInputLost`]: focus that depended on it is returned to
//! its owner and held input is released), and changes to the
//! screen, layout, or [`Config`]. [`Engine::handle`] returns the [`Decision`]s
//! that follow. The engine never reads a clock, starts a thread, or does I/O:
//! the same events always produce the same decisions.
//!
//! # Focus states
//!
//! | State | Meaning | [`CaptureMode`] |
//! |---|---|---|
//! | `Local` | This machine's input stays here. | `PassAll` |
//! | `Forwarding(peer)` | This machine's input goes to `peer`; the cursor waits at its exit point. | `WithholdAll` |
//! | `Controlled(peer)` | `peer`'s input is injected here; local mouse input is withheld while the takeover rules run. | `WithholdMouse` |
//!
//! The agent's hook reads the capture mode as a flag; it never waits for the
//! engine (hook procedures may only enqueue).
//!
//! # Layout learning
//!
//! The engine keeps a [`LayoutBook`]: one layout per
//! monitor set, and the current set's layout drives crossing.
//!
//! - **Learning:** an unmapped outer side learns its peer when exactly one
//!   connected peer is unplaced and the user pushes deliberately
//!   (`push_units`, whatever the sensitivity). Every monitor side that is at
//!   least partly outer in that direction is filled at once, and the engine
//!   emits [`Decision::OfferUndo`].
//! - **Reverse edge:** the machine the cursor enters learns the side toward
//!   `from` in [`PeerMessage::FocusEnter`], if that side is free and `from` is
//!   unplaced there.
//! - **Undo:** [`Event::Undo`] within `undo_ms` removes the side here and, via
//!   [`PeerMessage::ForgetSide`], on the peer, and withdraws focus from the
//!   peer if it is still there.
//! - **Forget layout:** [`Event::ForgetLayout`] empties the book.
//! - **Monitor sets:** a monitor set seen for the first time inherits each
//!   side's peer from the previous set.
//!
//! Every change is reported as [`Decision::LayoutsChanged`] for storage.
//! [`Event::LayoutsLoaded`] restores the stored book at start-up.
//!
//! # Tuning
//!
//! Every tunable number is in [`Tuning::DEFAULT`]:
//!
//! | Value | Default | Rule |
//! |---|---|---|
//! | `corner_tenths_mm` | 20 (2 mm) | Corner zones at both ends of each monitor side never cross. |
//! | `push_units` | 20 | Outward raw movement needed to cross with [`EdgeSensitivity::Push`], and always needed to learn a side. |
//! | `push_pause_ms` | 250 | A pause longer than this resets the push count. |
//! | `burst_gap_ms` | 150 | Local movement after a longer stillness starts a new burst. |
//! | `takeover_ms` | 2000 | A second burst within this long after the first, or one burst this long, takes control back. |
//! | `undo_ms` | 10000 | How long after learning a side Undo still works. |

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

pub use model::CaptureMode;
use model::geometry::corner_zone_px;
use model::{
    Button, EdgeFraction, InputAction, InputEvent, Key, Layout, LayoutBook, Millis, MonitorId,
    MonitorSetKey, Origin, PeerId, PeerMessage, Point, Screen, Side,
};

/// Every tunable number the engine uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tuning {
    pub corner_tenths_mm: u32,
    pub push_units: i32,
    pub push_pause_ms: u64,
    pub burst_gap_ms: u64,
    pub takeover_ms: u64,
    pub undo_ms: u64,
    /// Longest gap between the two Scroll Lock presses that bring input home.
    pub home_gap_ms: u64,
}

impl Tuning {
    pub const DEFAULT: Tuning = Tuning {
        corner_tenths_mm: 20,
        push_units: 20,
        push_pause_ms: 250,
        burst_gap_ms: 150,
        takeover_ms: 2000,
        undo_ms: 10_000,
        home_gap_ms: 500,
    };
}

/// How readily the cursor crosses a mapped edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeSensitivity {
    /// Cross on the first outward push.
    #[default]
    Instant,
    /// Cross only after [`Tuning::push_units`] of outward movement.
    Push,
}

/// User settings the engine reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Config {
    pub sensitivity: EdgeSensitivity,
}

/// Where focus is, as seen from outside the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusView {
    Local,
    Forwarding(PeerId),
    Controlled(PeerId),
}

/// Something that happened, fed to [`Engine::handle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Local input as the agent saw it.
    Local {
        at: Millis,
        input: InputEvent,
        origin: Origin,
    },
    /// A message from a peer's engine.
    FromPeer {
        peer: PeerId,
        msg: PeerMessage,
    },
    PeerConnected(PeerId),
    PeerLost(PeerId),
    /// This machine's input agent ended: no local input arrives and nothing
    /// can be injected until a new one reports. Focus that depended on it
    /// returns to its owner, and the peer releases what it held.
    LocalInputLost,
    ScreenChanged(Screen),
    /// Sets the current monitor set's layout (tests, and later the
    /// arrangement view). Not reported back as a change.
    LayoutChanged(Layout),
    /// Layouts stored earlier, at start-up.
    LayoutsLoaded(LayoutBook),
    ConfigChanged(Config),
    /// The user chose Undo on the learning notice.
    Undo {
        at: Millis,
    },
    /// The user chose Forget layout.
    ForgetLayout,
}

/// What the host must do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Publish a new capture mode to the agent's hook.
    SetCapture(CaptureMode),
    /// Send a message to a peer's engine.
    Send { peer: PeerId, msg: PeerMessage },
    /// Inject input into this machine's OS.
    Inject(InputAction),
    /// Move this machine's cursor.
    WarpCursor(Point),
    /// Ask this machine to show its secure attention (Ctrl-Alt-Del) screen.
    RequestSecureAttention,
    /// Focus moved; for the UI and diagnostics.
    FocusChanged(FocusView),
    /// The layouts changed; store this book.
    LayoutsChanged(LayoutBook),
    /// Show the notice that `side` was learned toward `peer`, with Undo.
    OfferUndo { peer: PeerId, side: Side },
}

/// Keys and buttons held down somewhere as a result of one input stream.
#[derive(Debug, Clone, Default)]
struct Held {
    keys: BTreeSet<Key>,
    buttons: BTreeSet<Button>,
}

impl Held {
    /// Records `a`; returns false for a release with no matching press.
    fn note(&mut self, a: InputAction) -> bool {
        match a {
            InputAction::Key { key, down: true } => {
                self.keys.insert(key);
                true
            }
            InputAction::Key { key, down: false } => self.keys.remove(&key),
            InputAction::Button { button, down: true } => {
                self.buttons.insert(button);
                true
            }
            InputAction::Button {
                button,
                down: false,
            } => self.buttons.remove(&button),
            InputAction::Wheel { .. } | InputAction::Motion { .. } => true,
        }
    }

    /// Releases for everything held, emptying the set.
    fn releases(&mut self) -> impl Iterator<Item = InputAction> + use<> {
        let keys = std::mem::take(&mut self.keys)
            .into_iter()
            .map(|key| InputAction::Key { key, down: false });
        let buttons =
            std::mem::take(&mut self.buttons)
                .into_iter()
                .map(|button| InputAction::Button {
                    button,
                    down: false,
                });
        keys.chain(buttons)
    }

    fn clear(&mut self) {
        self.keys.clear();
        self.buttons.clear();
    }
}

/// Local mouse activity on a controlled machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Contest {
    /// Last local mouse activity.
    last: Option<Millis>,
    /// Start of the current burst.
    burst_start: Millis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Local,
    Forwarding { to: PeerId, exit: Point },
    Controlled { by: PeerId, contest: Contest },
}

/// Outward movement accumulated against one edge, for [`EdgeSensitivity::Push`].
#[derive(Debug, Clone)]
struct Push {
    monitor: MonitorId,
    side: Side,
    total: i32,
    last: Millis,
}

/// A crossing the rules allow: toward `peer` through `side` at `fraction`.
struct Crossing {
    peer: PeerId,
    side: Side,
    fraction: EdgeFraction,
}

/// The most recent learned side, which Undo can take back.
#[derive(Debug, Clone, Copy)]
struct Learned {
    peer: PeerId,
    side: Side,
    at: Millis,
}

/// Focus and routing for one machine.
#[derive(Debug, Clone)]
pub struct Engine {
    local: PeerId,
    config: Config,
    tuning: Tuning,
    screen: Screen,
    /// Layouts for every monitor set seen; `layout` is the current set's.
    book: LayoutBook,
    set_key: MonitorSetKey,
    layout: Layout,
    undo: Option<Learned>,
    connected: BTreeSet<PeerId>,
    focus: Focus,
    /// Physical keys currently down on this machine's keyboard.
    physical_keys: BTreeSet<Key>,
    /// Held on this machine's OS by local physical input while focus was local.
    pressed_local: Held,
    /// Forwarded as pressed to the peer this machine forwards to.
    forwarded: Held,
    /// Injected as pressed here on the controlling peer's behalf.
    injected: Held,
    push: Option<Push>,
    /// When the last Scroll Lock press arrived while forwarding; a second
    /// within `home_gap_ms` brings input home. Only meaningful while
    /// forwarding, and cleared on every focus change.
    last_home_key: Option<Millis>,
    out: Vec<Decision>,
}

impl Engine {
    /// An engine for the machine whose identity is `local`.
    pub fn new(local: PeerId, config: Config) -> Self {
        Self {
            local,
            config,
            tuning: Tuning::DEFAULT,
            screen: Screen::default(),
            book: LayoutBook::default(),
            set_key: MonitorSetKey::default(),
            layout: Layout::default(),
            undo: None,
            connected: BTreeSet::new(),
            focus: Focus::Local,
            physical_keys: BTreeSet::new(),
            pressed_local: Held::default(),
            forwarded: Held::default(),
            injected: Held::default(),
            push: None,
            last_home_key: None,
            out: Vec::new(),
        }
    }

    /// Where focus is now.
    pub fn focus(&self) -> FocusView {
        match self.focus {
            Focus::Local => FocusView::Local,
            Focus::Forwarding { to, .. } => FocusView::Forwarding(to),
            Focus::Controlled { by, .. } => FocusView::Controlled(by),
        }
    }

    /// Handles one event and returns the decisions it leads to, in order.
    pub fn handle(&mut self, event: Event) -> Vec<Decision> {
        match event {
            Event::Local { at, input, origin } => self.on_local(at, input, origin),
            Event::FromPeer { peer, msg } => self.on_peer(peer, msg),
            Event::PeerConnected(peer) => {
                self.connected.insert(peer);
            }
            Event::PeerLost(peer) => self.on_peer_lost(peer),
            Event::LocalInputLost => self.on_local_input_lost(),
            Event::ScreenChanged(screen) => self.on_screen(screen),
            Event::LayoutChanged(layout) => {
                self.book.set(self.set_key.clone(), layout.clone());
                self.layout = layout;
                self.push = None;
            }
            Event::LayoutsLoaded(book) => {
                self.layout = book.get(&self.set_key).cloned().unwrap_or_default();
                self.book = book;
                self.push = None;
            }
            Event::ConfigChanged(config) => {
                self.config = config;
                self.push = None;
            }
            Event::Undo { at } => self.on_undo(at),
            Event::ForgetLayout => {
                self.book = LayoutBook::default();
                self.layout = Layout::default();
                self.undo = None;
                self.push = None;
                self.out.push(Decision::LayoutsChanged(self.book.clone()));
            }
        }
        std::mem::take(&mut self.out)
    }

    /// The current monitor set's layout.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    // ---- layouts ----

    /// Switches to the new monitor set's layout, building a first-seen set's
    /// layout from the previous set's sides.
    fn on_screen(&mut self, screen: Screen) {
        let key = screen.set_key();
        self.screen = screen;
        self.push = None;
        if key == self.set_key {
            return;
        }
        self.set_key = key;
        if let Some(known) = self.book.get(&self.set_key) {
            self.layout = known.clone();
            return;
        }
        let previous = std::mem::take(&mut self.layout);
        for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            if let Some(peer) = previous.main_peer(side) {
                self.fill(side, peer);
            }
        }
        if !self.layout.is_empty() {
            self.save();
        }
    }

    /// Maps `side` of every at-least-partly-outer monitor to `peer`.
    fn fill(&mut self, side: Side, peer: PeerId) {
        let ids: Vec<MonitorId> = self
            .screen
            .outer_sides(side)
            .into_iter()
            .map(|m| m.id.clone())
            .collect();
        for id in ids {
            self.layout.set(id, side, peer);
        }
    }

    /// Stores the current layout in the book and reports the book.
    fn save(&mut self) {
        self.book.set(self.set_key.clone(), self.layout.clone());
        self.out.push(Decision::LayoutsChanged(self.book.clone()));
    }

    /// The one connected peer placed on no side, if there is exactly one.
    fn sole_unplaced(&self) -> Option<PeerId> {
        let mut unplaced = self
            .connected
            .iter()
            .copied()
            .filter(|p| !self.layout.places(*p));
        let first = unplaced.next()?;
        unplaced.next().is_none().then_some(first)
    }

    /// Learns `side` toward `peer` after a push, with Undo on offer.
    fn learn(&mut self, at: Millis, side: Side, peer: PeerId) {
        self.fill(side, peer);
        self.save();
        self.undo = Some(Learned { peer, side, at });
        self.out.push(Decision::OfferUndo { peer, side });
    }

    /// Learns the side the cursor entered through toward the machine whose
    /// edge it left, if that side is free and that machine is unplaced here.
    fn learn_reverse(&mut self, side: Side, from: PeerId) {
        if from == self.local || self.layout.places(from) {
            return;
        }
        let sides = self.screen.outer_sides(side);
        let free = !sides.is_empty()
            && sides
                .iter()
                .all(|m| self.layout.peer(&m.id, side).is_none());
        if free {
            self.fill(side, from);
            self.save();
        }
    }

    fn on_undo(&mut self, at: Millis) {
        let Some(learned) = self.undo.take() else {
            return;
        };
        if at.since(learned.at) > self.tuning.undo_ms {
            return;
        }
        if self.layout.forget(learned.side, learned.peer) {
            self.save();
        }
        if let Focus::Forwarding { to, exit } = self.focus
            && to == learned.peer
        {
            self.withdraw_to_local(to, exit);
        }
        self.send(
            learned.peer,
            PeerMessage::ForgetSide {
                side: learned.side.opposite(),
            },
        );
    }

    // ---- local input ----

    fn on_local(&mut self, at: Millis, input: InputEvent, origin: Origin) {
        if origin == Origin::Physical
            && let InputEvent::Key { key, down } = input
        {
            if down {
                self.physical_keys.insert(key);
            } else {
                self.physical_keys.remove(&key);
            }
        }
        match (self.focus, origin) {
            (Focus::Local, Origin::Physical) => self.local_input(at, input),
            (Focus::Forwarding { to, exit }, Origin::Physical) => self.forward(at, to, exit, input),
            (Focus::Controlled { .. }, Origin::Physical) => self.contested_input(at, input),
            (Focus::Controlled { by, .. }, Origin::Injected) => {
                if let InputEvent::Motion { dx, dy, cursor } = input {
                    self.controlled_motion(at, by, dx, dy, cursor);
                }
            }
            // Injected input never crosses, forwards, or takes over.
            (Focus::Local | Focus::Forwarding { .. }, Origin::Injected) => {}
        }
    }

    /// Physical input while focus is local.
    fn local_input(&mut self, at: Millis, input: InputEvent) {
        match input {
            InputEvent::Key { key, down } => {
                self.pressed_local.note(InputAction::Key { key, down });
            }
            InputEvent::Button { button, down } => {
                self.pressed_local
                    .note(InputAction::Button { button, down });
            }
            InputEvent::Wheel { .. } => {}
            InputEvent::Motion { dx, dy, cursor } => {
                let button_held = !self.pressed_local.buttons.is_empty();
                if let Some(c) = self.crossing(at, cursor, dx, dy, button_held) {
                    let releases: Vec<_> = self.pressed_local.releases().collect();
                    self.out.extend(releases.into_iter().map(Decision::Inject));
                    self.forwarded.clear();
                    self.send(
                        c.peer,
                        PeerMessage::FocusEnter {
                            side: c.side.opposite(),
                            fraction: c.fraction,
                            from: self.local,
                        },
                    );
                    self.set_focus(
                        Focus::Forwarding {
                            to: c.peer,
                            exit: cursor,
                        },
                        None,
                    );
                }
            }
        }
    }

    /// Physical input while forwarding to `to`, whose entry the cursor left
    /// at `exit`.
    fn forward(&mut self, at: Millis, to: PeerId, exit: Point, input: InputEvent) {
        let action = match input {
            InputEvent::Key {
                key: Key::END,
                down: true,
            } if self.secure_attention_chord() => {
                self.send(to, PeerMessage::SecureAttention);
                return;
            }
            // Scroll Lock never reaches the peer; a second press within the
            // window brings input home from this keyboard alone.
            InputEvent::Key {
                key: Key::SCROLL_LOCK,
                down,
            } => {
                if down {
                    match self.last_home_key {
                        Some(last) if at.since(last) <= self.tuning.home_gap_ms => {
                            self.withdraw_to_local(to, exit);
                        }
                        _ => self.last_home_key = Some(at),
                    }
                }
                return;
            }
            InputEvent::Key { key, down } => InputAction::Key { key, down },
            InputEvent::Button { button, down } => InputAction::Button { button, down },
            InputEvent::Wheel { dx, dy } => InputAction::Wheel { dx, dy },
            InputEvent::Motion { dx, dy, .. } => InputAction::Motion { dx, dy },
        };
        if self.forwarded.note(action) {
            self.send(to, PeerMessage::Input(action));
        }
    }

    fn secure_attention_chord(&self) -> bool {
        self.physical_keys.iter().any(|k| k.is_ctrl())
            && self.physical_keys.iter().any(|k| k.is_alt())
    }

    /// Returns focus to this machine at `exit`, releasing what the peer held:
    /// `FocusWithdrawn` tells the peer to release and take focus back, and the
    /// return home does not wait for it.
    fn withdraw_to_local(&mut self, to: PeerId, exit: Point) {
        self.forwarded.clear();
        self.send(to, PeerMessage::FocusWithdrawn);
        self.set_focus(Focus::Local, Some(exit));
    }

    /// Physical input while a peer controls this machine.
    fn contested_input(&mut self, at: Millis, input: InputEvent) {
        match input {
            InputEvent::Key { key, down } => {
                self.pressed_local.note(InputAction::Key { key, down });
                if down {
                    self.take_back();
                }
            }
            InputEvent::Button { .. } | InputEvent::Wheel { .. } | InputEvent::Motion { .. } => {
                self.local_mouse_activity(at);
            }
        }
    }

    /// Groups local mouse activity into bursts and takes focus back on a
    /// second burst soon after the first, or on one long burst.
    fn local_mouse_activity(&mut self, at: Millis) {
        let t = self.tuning;
        let Focus::Controlled { contest, .. } = &mut self.focus else {
            return;
        };
        let take = match contest.last {
            Some(last) if at.since(last) <= t.burst_gap_ms => {
                at.since(contest.burst_start) >= t.takeover_ms
            }
            Some(last) if at.since(last) <= t.takeover_ms => true,
            Some(_) | None => {
                contest.burst_start = at;
                false
            }
        };
        contest.last = Some(at);
        if take {
            self.take_back();
        }
    }

    /// This machine's own user takes focus back from its controller.
    fn take_back(&mut self) {
        let Focus::Controlled { by, .. } = self.focus else {
            return;
        };
        let releases: Vec<_> = self.injected.releases().collect();
        self.out.extend(releases.into_iter().map(Decision::Inject));
        self.send(by, PeerMessage::TakenBack);
        self.set_focus(Focus::Local, None);
    }

    /// Injected motion while controlled by `by`: this machine's own edges.
    fn controlled_motion(&mut self, at: Millis, by: PeerId, dx: i32, dy: i32, cursor: Point) {
        let button_held = !self.injected.buttons.is_empty();
        if let Some(c) = self.crossing(at, cursor, dx, dy, button_held) {
            let releases: Vec<_> = self.injected.releases().collect();
            self.out.extend(releases.into_iter().map(Decision::Inject));
            self.send(
                by,
                PeerMessage::EdgeExit {
                    target: c.peer,
                    side: c.side,
                    fraction: c.fraction,
                },
            );
            self.set_focus(Focus::Local, None);
        }
    }

    /// Whether a movement to `cursor` by `(dx, dy)` crosses an edge now.
    fn crossing(
        &mut self,
        at: Millis,
        cursor: Point,
        dx: i32,
        dy: i32,
        button_held: bool,
    ) -> Option<Crossing> {
        let candidate = [Side::Left, Side::Right, Side::Top, Side::Bottom]
            .into_iter()
            .find_map(|side| {
                let amount = side.outward(dx, dy);
                if amount <= 0 || button_held {
                    return None;
                }
                let m = self.screen.outer_edge(cursor, side)?;
                let zone = corner_zone_px(m.dpi, self.tuning.corner_tenths_mm);
                if m.in_corner(cursor, side, zone) {
                    return None;
                }
                match self.layout.peer(&m.id, side) {
                    Some(peer) => self
                        .connected
                        .contains(&peer)
                        .then(|| (m.id.clone(), side, amount, peer, false)),
                    // Unmapped: learnable only toward the one unplaced peer.
                    None => self
                        .sole_unplaced()
                        .map(|peer| (m.id.clone(), side, amount, peer, true)),
                }
            });
        let Some((monitor, side, amount, peer, learning)) = candidate else {
            self.push = None;
            return None;
        };

        // Learning always needs a deliberate push, whatever the sensitivity.
        if learning || self.config.sensitivity == EdgeSensitivity::Push {
            let total = match &self.push {
                Some(p)
                    if p.monitor == monitor
                        && p.side == side
                        && at.since(p.last) <= self.tuning.push_pause_ms =>
                {
                    p.total.saturating_add(amount)
                }
                _ => amount,
            };
            if total < self.tuning.push_units {
                self.push = Some(Push {
                    monitor,
                    side,
                    total,
                    last: at,
                });
                return None;
            }
        }
        self.push = None;
        if learning {
            self.learn(at, side, peer);
        }
        Some(Crossing {
            peer,
            side,
            fraction: self.screen.fraction_at(side, cursor),
        })
    }

    // ---- peers ----

    fn on_peer(&mut self, peer: PeerId, msg: PeerMessage) {
        match (self.focus, msg) {
            (
                Focus::Local,
                PeerMessage::FocusEnter {
                    side,
                    fraction,
                    from,
                },
            ) => {
                self.injected.clear();
                let warp = self.screen.entry_point(side, fraction);
                self.set_focus(
                    Focus::Controlled {
                        by: peer,
                        contest: Contest::default(),
                    },
                    warp,
                );
                self.learn_reverse(side, from);
            }
            (_, PeerMessage::FocusEnter { .. }) => self.send(peer, PeerMessage::FocusRefused),

            (Focus::Controlled { by, .. }, PeerMessage::FocusWithdrawn) if by == peer => {
                let releases: Vec<_> = self.injected.releases().collect();
                self.out.extend(releases.into_iter().map(Decision::Inject));
                self.set_focus(Focus::Local, None);
            }
            (_, PeerMessage::ForgetSide { side }) => {
                if self.layout.forget(side, peer) {
                    self.save();
                }
            }

            (
                Focus::Forwarding { to, exit },
                PeerMessage::FocusRefused | PeerMessage::TakenBack,
            ) if to == peer => {
                self.forwarded.clear();
                self.set_focus(Focus::Local, Some(exit));
            }
            (
                Focus::Forwarding { to, exit },
                PeerMessage::EdgeExit {
                    target,
                    side,
                    fraction,
                },
            ) if to == peer => self.follow_exit(peer, exit, target, side, fraction),

            (Focus::Controlled { by, .. }, PeerMessage::Input(action)) if by == peer => {
                if self.injected.note(action) {
                    self.out.push(Decision::Inject(action));
                }
            }
            (Focus::Controlled { by, .. }, PeerMessage::SecureAttention) if by == peer => {
                self.out.push(Decision::RequestSecureAttention);
            }
            // Input from a peer that does not control us, or stale messages.
            _ => {}
        }
    }

    /// The machine this one forwards to reported the cursor leaving it.
    fn follow_exit(
        &mut self,
        from: PeerId,
        exit: Point,
        target: PeerId,
        side: Side,
        fraction: EdgeFraction,
    ) {
        // `from` released everything it held for us when the cursor left.
        self.forwarded.clear();
        if target == self.local {
            let warp = self.screen.entry_point(side.opposite(), fraction);
            self.set_focus(Focus::Local, warp);
        } else if self.connected.contains(&target) {
            self.send(
                target,
                PeerMessage::FocusEnter {
                    side: side.opposite(),
                    fraction,
                    from,
                },
            );
            self.set_focus(Focus::Forwarding { to: target, exit }, None);
        } else {
            // Unreachable from here: put the cursor back where it left `from`.
            self.send(
                from,
                PeerMessage::FocusEnter {
                    side,
                    fraction,
                    from,
                },
            );
        }
    }

    fn on_peer_lost(&mut self, peer: PeerId) {
        self.connected.remove(&peer);
        self.push = None;
        match self.focus {
            Focus::Forwarding { to, exit } if to == peer => {
                self.forwarded.clear();
                self.set_focus(Focus::Local, Some(exit));
            }
            Focus::Controlled { by, .. } if by == peer => {
                let releases: Vec<_> = self.injected.releases().collect();
                self.out.extend(releases.into_iter().map(Decision::Inject));
                self.set_focus(Focus::Local, None);
            }
            _ => {}
        }
    }

    /// The agent is gone: a peer we forward to withdraws and releases, a
    /// peer controlling us gets its focus back, and nothing held locally is
    /// remembered, since its release will never be reported.
    fn on_local_input_lost(&mut self) {
        self.physical_keys.clear();
        self.pressed_local.clear();
        self.push = None;
        match self.focus {
            Focus::Forwarding { to, .. } => {
                self.forwarded.clear();
                self.send(to, PeerMessage::FocusWithdrawn);
                self.set_focus(Focus::Local, None);
            }
            Focus::Controlled { .. } => self.take_back(),
            Focus::Local => {}
        }
    }

    // ---- output ----

    fn send(&mut self, peer: PeerId, msg: PeerMessage) {
        self.out.push(Decision::Send { peer, msg });
    }

    fn set_focus(&mut self, focus: Focus, warp: Option<Point>) {
        let before = (self.capture(), self.focus());
        self.focus = focus;
        self.push = None;
        self.last_home_key = None;
        if let Some(p) = warp {
            self.out.push(Decision::WarpCursor(p));
        }
        if self.capture() != before.0 {
            self.out.push(Decision::SetCapture(self.capture()));
        }
        if self.focus() != before.1 {
            self.out.push(Decision::FocusChanged(self.focus()));
        }
    }

    fn capture(&self) -> CaptureMode {
        match self.focus {
            Focus::Local => CaptureMode::PassAll,
            Focus::Forwarding { .. } => CaptureMode::WithholdAll,
            Focus::Controlled { .. } => CaptureMode::WithholdMouse,
        }
    }
}
