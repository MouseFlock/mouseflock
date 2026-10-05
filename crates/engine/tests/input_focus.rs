//! Replay tests for `openspec/specs/input-focus`. Each test is named after the
//! spec scenario it checks.

mod replay;

use engine::{CaptureMode, Config, Decision, EdgeSensitivity, Event, FocusView};
use model::{Button, EdgeFraction, InputAction, Key, PeerMessage, Point, Screen, Side};
use replay::{A, B, C, Desk, button, hd, key, monitor, motion};

const KEY_X: Key = Key(0x1b);

fn fraction(screen: &Screen, side: Side, y: i32) -> EdgeFraction {
    screen.fraction_at(side, Point::new(0, y))
}

/// A and B side by side: A's right side leads to B, B's left side to A.
fn a_beside_b() -> Desk {
    let mut desk = Desk::new(&[(A, hd("A1")), (B, hd("B1"))]);
    desk.layout(A, &[("A1", Side::Right, B)]);
    desk.layout(B, &[("B1", Side::Left, A)]);
    desk
}

/// Pushes A's cursor out of its right side at height `y`.
fn cross_a_to_b(desk: &mut Desk, y: i32) -> replay::Outcome {
    let out = desk.physical(A, motion(5, 0, 1919, y));
    assert_eq!(desk.focus(A), FocusView::Forwarding(B), "{out:?}");
    out
}

fn a_controls_b() -> Desk {
    let mut desk = a_beside_b();
    cross_a_to_b(&mut desk, 540);
    desk
}

fn sends(out: &[Decision], msg: PeerMessage) -> bool {
    out.iter()
        .any(|d| matches!(d, Decision::Send { msg: m, .. } if *m == msg))
}

// ---- Focus follows the cursor ----

#[test]
fn input_stays_local() {
    let mut desk = a_beside_b();
    let out = desk.physical(A, key(KEY_X, true));
    assert!(out.is_quiet(), "{out:?}");
}

#[test]
fn input_goes_to_the_focused_peer() {
    let mut desk = a_controls_b();
    let down = InputAction::Key {
        key: KEY_X,
        down: true,
    };
    let out = desk.physical(A, key(KEY_X, true));
    assert_eq!(
        out.of(A),
        [Decision::Send {
            peer: B,
            msg: PeerMessage::Input(down)
        }]
    );
    assert_eq!(out.of(B), [Decision::Inject(down)]);
}

// ---- Physical and injected input are distinguished ----

#[test]
fn injected_motion_does_not_take_over() {
    let mut desk = a_controls_b();
    for t in (1000..=4000).step_by(100) {
        let out = desk.at(t).injected(B, motion(3, 0, 900, 500));
        assert!(!sends(out.of(B), PeerMessage::TakenBack), "{out:?}");
    }
    assert_eq!(desk.focus(B), FocusView::Controlled(A));
}

// ---- Forwarded motion is raw movement ----

#[test]
fn different_pointer_speeds() {
    let mut desk = a_controls_b();
    let raw = InputAction::Motion { dx: 7, dy: -3 };
    let out = desk.physical(A, motion(7, -3, 1919, 540));
    assert_eq!(
        out.of(A),
        [Decision::Send {
            peer: B,
            msg: PeerMessage::Input(raw)
        }]
    );
    assert_eq!(out.of(B), [Decision::Inject(raw)]);
}

// ---- Crossing a mapped edge ----

#[test]
fn instant_crossing() {
    let mut desk = a_beside_b();
    let out = cross_a_to_b(&mut desk, 540);
    assert_eq!(
        out.of(A),
        [
            Decision::Send {
                peer: B,
                msg: PeerMessage::FocusEnter {
                    side: Side::Left,
                    fraction: fraction(&hd("A1"), Side::Right, 540),
                    from: A,
                },
            },
            Decision::SetCapture(CaptureMode::WithholdAll),
            Decision::FocusChanged(FocusView::Forwarding(B)),
        ]
    );
    assert_eq!(
        out.of(B),
        [
            Decision::WarpCursor(Point::new(0, 540)),
            Decision::SetCapture(CaptureMode::WithholdMouse),
            Decision::FocusChanged(FocusView::Controlled(A)),
        ]
    );
}

#[test]
fn inner_side_is_not_an_edge() {
    let two = Screen::new(vec![
        monitor("A1", 0, 0, 1920, 1080, 96),
        monitor("A2", 1920, 0, 1920, 1080, 96),
    ]);
    let mut desk = Desk::new(&[(A, two), (B, hd("B1"))]);
    desk.layout(A, &[("A1", Side::Right, B)]);
    let out = desk.physical(A, motion(5, 0, 1919, 540));
    assert!(out.is_quiet(), "{out:?}");
    assert_eq!(desk.focus(A), FocusView::Local);
}

// ---- Edges that act as walls ----

#[test]
fn dragging_at_the_edge() {
    let mut desk = a_beside_b();
    desk.physical(A, button(Button::Left, true));
    let out = desk.physical(A, motion(5, 0, 1919, 540));
    assert!(out.is_quiet(), "{out:?}");
    assert_eq!(desk.focus(A), FocusView::Local);
}

#[test]
fn peer_not_connected() {
    let mut desk = a_beside_b();
    desk.lose(A, B);
    let out = desk.physical(A, motion(5, 0, 1919, 540));
    assert!(out.is_quiet(), "{out:?}");
    assert_eq!(desk.focus(A), FocusView::Local);
}

// ---- Corner zones never cross ----

#[test]
fn closing_a_maximized_window() {
    let mut desk = a_beside_b();
    let out = desk.physical(A, motion(5, -5, 1919, 0));
    assert!(out.is_quiet(), "{out:?}");
    assert_eq!(desk.focus(A), FocusView::Local);
}

#[test]
fn corner_size_follows_dpi() {
    let hidpi = Screen::new(vec![monitor("A1", 0, 0, 3840, 2160, 192)]);
    let mut desk = Desk::new(&[(A, hidpi), (B, hd("B1"))]);
    desk.layout(A, &[("A1", Side::Right, B)]);
    // 2 mm at 192 DPI is 15 px: y = 14 is in the zone, y = 15 is not.
    assert!(desk.physical(A, motion(5, 0, 3839, 14)).is_quiet());
    assert_eq!(desk.focus(A), FocusView::Local);
    desk.physical(A, motion(5, 0, 3839, 15));
    assert_eq!(desk.focus(A), FocusView::Forwarding(B));
}

// ---- Edge sensitivity setting ----

fn push_desk() -> Desk {
    let mut desk = a_beside_b();
    desk.config(
        A,
        Config {
            sensitivity: EdgeSensitivity::Push,
        },
    );
    desk
}

#[test]
fn push_sensitivity_needs_a_deliberate_push() {
    let mut desk = push_desk();
    desk.at(0).physical(A, motion(6, 0, 1919, 540));
    desk.at(10).physical(A, motion(6, 0, 1919, 540));
    assert_eq!(desk.focus(A), FocusView::Local);
}

#[test]
fn push_sensitivity_crosses_on_a_sustained_push() {
    let mut desk = push_desk();
    for t in [0, 10, 20] {
        desk.at(t).physical(A, motion(5, 0, 1919, 540));
        assert_eq!(desk.focus(A), FocusView::Local);
    }
    // 20 units reached.
    desk.at(30).physical(A, motion(5, 0, 1919, 540));
    assert_eq!(desk.focus(A), FocusView::Forwarding(B));
}

#[test]
fn pause_resets_the_push() {
    let mut desk = push_desk();
    for t in [0, 10, 20] {
        desk.at(t).physical(A, motion(5, 0, 1919, 540));
    }
    for t in [320, 330] {
        desk.at(t).physical(A, motion(5, 0, 1919, 540));
    }
    assert_eq!(desk.focus(A), FocusView::Local);
}

// ---- Entry position ----

#[test]
fn same_relative_height() {
    let tall = Screen::new(vec![monitor("B1", 0, 0, 3840, 2160, 192)]);
    let mut desk = Desk::new(&[(A, hd("A1")), (B, tall)]);
    desk.layout(A, &[("A1", Side::Right, B)]);
    // 270 of 1080 is 25%; 25% of 2160 is 540.
    let out = cross_a_to_b(&mut desk, 270);
    assert_eq!(out.of(B)[0], Decision::WarpCursor(Point::new(0, 540)));
}

// ---- Crossing out of a controlled machine ----

#[test]
fn back_to_the_controlling_machine() {
    let mut desk = a_controls_b();
    // 648 of 1080 is 60%.
    let out = desk.injected(B, motion(-5, 0, 0, 648));
    assert_eq!(
        out.of(B),
        [
            Decision::Send {
                peer: A,
                msg: PeerMessage::EdgeExit {
                    target: A,
                    side: Side::Left,
                    fraction: fraction(&hd("B1"), Side::Left, 648),
                },
            },
            Decision::SetCapture(CaptureMode::PassAll),
            Decision::FocusChanged(FocusView::Local),
        ]
    );
    assert_eq!(
        out.of(A),
        [
            Decision::WarpCursor(Point::new(1919, 648)),
            Decision::SetCapture(CaptureMode::PassAll),
            Decision::FocusChanged(FocusView::Local),
        ]
    );
}

fn a_b_c_in_a_row() -> Desk {
    let mut desk = Desk::new(&[(A, hd("A1")), (B, hd("B1")), (C, hd("C1"))]);
    desk.layout(A, &[("A1", Side::Right, B)]);
    desk.layout(B, &[("B1", Side::Left, A), ("B1", Side::Right, C)]);
    desk.layout(C, &[("C1", Side::Left, B)]);
    desk
}

#[test]
fn on_to_a_third_machine() {
    let mut desk = a_b_c_in_a_row();
    cross_a_to_b(&mut desk, 540);
    let out = desk.injected(B, motion(5, 0, 1919, 300));
    assert_eq!(desk.focus(A), FocusView::Forwarding(C));
    assert_eq!(desk.focus(B), FocusView::Local);
    assert_eq!(desk.focus(C), FocusView::Controlled(A));
    assert_eq!(out.of(C)[0], Decision::WarpCursor(Point::new(0, 300)));

    // A's input now goes to C.
    let out = desk.physical(A, key(KEY_X, true));
    assert!(out.of(C).contains(&Decision::Inject(InputAction::Key {
        key: KEY_X,
        down: true
    })));
    assert!(out.of(B).is_empty());
}

#[test]
fn third_machine_unreachable_acts_as_wall() {
    let mut desk = a_b_c_in_a_row();
    desk.lose(A, C);
    cross_a_to_b(&mut desk, 540);
    let out = desk.injected(B, motion(5, 0, 1919, 300));
    assert_eq!(desk.focus(A), FocusView::Forwarding(B));
    assert_eq!(desk.focus(B), FocusView::Controlled(A));
    assert!(
        out.of(B)
            .contains(&Decision::WarpCursor(Point::new(1919, 300)))
    );
}

// ---- One controller at a time ----

#[test]
fn second_controller_refused() {
    let mut desk = Desk::new(&[(A, hd("A1")), (B, hd("B1")), (C, hd("C1"))]);
    desk.layout(A, &[("A1", Side::Right, B)]);
    desk.layout(C, &[("C1", Side::Left, B)]);
    cross_a_to_b(&mut desk, 540);

    let out = desk.physical(C, motion(-5, 0, 0, 500));
    assert!(sends(out.of(B), PeerMessage::FocusRefused));
    assert_eq!(desk.focus(C), FocusView::Local);
    assert!(
        out.of(C)
            .contains(&Decision::WarpCursor(Point::new(0, 500)))
    );
    assert_eq!(desk.focus(A), FocusView::Forwarding(B));
    assert_eq!(desk.focus(B), FocusView::Controlled(A));
}

// ---- Focus changes release held input ----

#[test]
fn crossing_with_shift_held() {
    let mut desk = a_beside_b();
    desk.physical(A, key(Key::LEFT_SHIFT, true));
    let out = cross_a_to_b(&mut desk, 540);
    assert_eq!(
        out.of(A)[0],
        Decision::Inject(InputAction::Key {
            key: Key::LEFT_SHIFT,
            down: false
        })
    );
    assert!(
        !out.of(B)
            .iter()
            .any(|d| matches!(d, Decision::Inject(InputAction::Key { .. })))
    );
}

#[test]
fn release_after_the_crossing() {
    let mut desk = a_beside_b();
    desk.physical(A, key(Key::LEFT_CTRL, true));
    cross_a_to_b(&mut desk, 540);
    let out = desk.physical(A, key(Key::LEFT_CTRL, false));
    assert!(out.is_quiet(), "{out:?}");
}

#[test]
fn leaving_a_controlled_machine_with_a_key_held() {
    let mut desk = a_controls_b();
    desk.physical(A, key(Key::LEFT_ALT, true));
    let out = desk.injected(B, motion(-5, 0, 0, 540));
    assert_eq!(
        out.of(B)[0],
        Decision::Inject(InputAction::Key {
            key: Key::LEFT_ALT,
            down: false
        })
    );
    assert_eq!(desk.focus(A), FocusView::Local);
}

// ---- Taking control back locally ----

fn nudge(desk: &mut Desk, from: u64) {
    for t in [from, from + 10, from + 20] {
        let out = desk.at(t).physical(B, motion(2, 1, 900, 500));
        assert!(!sends(out.of(B), PeerMessage::TakenBack), "{out:?}");
    }
}

#[test]
fn accidental_nudge() {
    let mut desk = a_beside_b();
    let out = cross_a_to_b(&mut desk, 540);
    // While controlled, the hook withholds B's own mouse input.
    assert!(
        out.of(B)
            .contains(&Decision::SetCapture(CaptureMode::WithholdMouse))
    );
    nudge(&mut desk, 1000);
    assert_eq!(desk.focus(B), FocusView::Controlled(A));
    assert_eq!(desk.focus(A), FocusView::Forwarding(B));
}

#[test]
fn second_movement_takes_over() {
    let mut desk = a_controls_b();
    nudge(&mut desk, 1000);
    let out = desk.at(2020).physical(B, motion(2, 1, 900, 500));
    assert_eq!(
        out.of(B),
        [
            Decision::Send {
                peer: A,
                msg: PeerMessage::TakenBack
            },
            Decision::SetCapture(CaptureMode::PassAll),
            Decision::FocusChanged(FocusView::Local),
        ]
    );
    assert_eq!(desk.focus(A), FocusView::Local);
}

#[test]
fn sustained_movement_takes_over() {
    let mut desk = a_controls_b();
    for t in (1000..3000).step_by(50) {
        desk.at(t).physical(B, motion(2, 1, 900, 500));
        assert_eq!(desk.focus(B), FocusView::Controlled(A), "at {t} ms");
    }
    desk.at(3000).physical(B, motion(2, 1, 900, 500));
    assert_eq!(desk.focus(B), FocusView::Local);
    assert_eq!(desk.focus(A), FocusView::Local);
}

#[test]
fn burst_after_the_window_is_a_first_burst_again() {
    let mut desk = a_controls_b();
    nudge(&mut desk, 1000);
    // 2.5 s after the first burst ended: a first burst again.
    nudge(&mut desk, 3520);
    assert_eq!(desk.focus(B), FocusView::Controlled(A));
}

#[test]
fn key_press_takes_over() {
    let mut desk = a_controls_b();
    let out = desk.physical(B, key(KEY_X, true));
    assert!(sends(out.of(B), PeerMessage::TakenBack));
    assert_eq!(desk.focus(B), FocusView::Local);
    // The key is not withheld while controlled, so B's OS already has it.
    assert!(!out.of(B).iter().any(|d| matches!(d, Decision::Inject(_))));
}

// ---- Losing control returns the cursor ----

#[test]
fn controlled_machine_takes_over() {
    let mut desk = a_beside_b();
    cross_a_to_b(&mut desk, 540);
    let out = desk.physical(B, key(KEY_X, true));
    assert_eq!(
        out.of(A),
        [
            Decision::WarpCursor(Point::new(1919, 540)),
            Decision::SetCapture(CaptureMode::PassAll),
            Decision::FocusChanged(FocusView::Local),
        ]
    );
}

#[test]
fn peer_lost_while_focused() {
    let mut desk = a_controls_b();
    desk.physical(A, key(Key::LEFT_CTRL, true));
    let out = desk.lose(A, B);
    assert_eq!(
        out.of(A),
        [
            Decision::WarpCursor(Point::new(1919, 540)),
            Decision::SetCapture(CaptureMode::PassAll),
            Decision::FocusChanged(FocusView::Local),
        ]
    );
}

#[test]
fn controller_lost_while_controlled() {
    let mut desk = a_controls_b();
    desk.physical(A, key(KEY_X, true));
    let out = desk.lose(B, A);
    assert_eq!(
        out.of(B),
        [
            Decision::Inject(InputAction::Key {
                key: KEY_X,
                down: false
            }),
            Decision::SetCapture(CaptureMode::PassAll),
            Decision::FocusChanged(FocusView::Local),
        ]
    );
}

// ---- Secure attention request ----

#[test]
fn ctrl_alt_end_while_controlling_a_peer() {
    let mut desk = a_controls_b();
    desk.physical(A, key(Key::LEFT_CTRL, true));
    desk.physical(A, key(Key::LEFT_ALT, true));
    let out = desk.physical(A, key(Key::END, true));
    assert_eq!(
        out.of(A),
        [Decision::Send {
            peer: B,
            msg: PeerMessage::SecureAttention
        }]
    );
    assert_eq!(out.of(B), [Decision::RequestSecureAttention]);
    // The End release has no forwarded press, so it is dropped.
    assert!(desk.physical(A, key(Key::END, false)).is_quiet());
}

#[test]
fn ctrl_alt_end_with_local_focus() {
    let mut desk = a_beside_b();
    for k in [Key::LEFT_CTRL, Key::LEFT_ALT, Key::END] {
        let out = desk.physical(A, key(k, true));
        assert!(out.is_quiet(), "{out:?}");
    }
}

// ---- Losing local input ----

#[test]
fn agent_ends_while_forwarding() {
    let mut desk = a_controls_b();
    desk.physical(A, key(Key::LEFT_CTRL, true));
    let out = desk.event(A, Event::LocalInputLost);
    assert_eq!(
        out.of(A),
        [
            Decision::Send {
                peer: B,
                msg: PeerMessage::FocusWithdrawn
            },
            Decision::SetCapture(CaptureMode::PassAll),
            Decision::FocusChanged(FocusView::Local),
        ]
    );
    assert_eq!(
        out.of(B),
        [
            Decision::Inject(InputAction::Key {
                key: Key::LEFT_CTRL,
                down: false
            }),
            Decision::SetCapture(CaptureMode::PassAll),
            Decision::FocusChanged(FocusView::Local),
        ]
    );
    assert_eq!(desk.focus(A), FocusView::Local);
    assert_eq!(desk.focus(B), FocusView::Local);
}

#[test]
fn agent_ends_while_controlled() {
    let mut desk = a_controls_b();
    desk.physical(A, key(KEY_X, true));
    let out = desk.event(B, Event::LocalInputLost);
    assert_eq!(
        out.of(B),
        [
            Decision::Inject(InputAction::Key {
                key: KEY_X,
                down: false
            }),
            Decision::Send {
                peer: A,
                msg: PeerMessage::TakenBack
            },
            Decision::SetCapture(CaptureMode::PassAll),
            Decision::FocusChanged(FocusView::Local),
        ]
    );
    // A gets focus back with its cursor where it left.
    assert_eq!(
        out.of(A),
        [
            Decision::WarpCursor(Point::new(1919, 540)),
            Decision::SetCapture(CaptureMode::PassAll),
            Decision::FocusChanged(FocusView::Local),
        ]
    );
}

#[test]
fn agent_ends_with_local_focus() {
    let mut desk = a_beside_b();
    desk.physical(A, key(KEY_X, true));
    let out = desk.event(A, Event::LocalInputLost);
    assert!(out.is_quiet(), "{out:?}");
}

#[test]
fn release_after_relink_is_dropped() {
    // Ctrl is held while forwarding, the agent ends, and a new agent reports
    // Ctrl's release after the cursor crossed again: B never saw this press.
    let mut desk = a_controls_b();
    desk.physical(A, key(Key::LEFT_CTRL, true));
    desk.event(A, Event::LocalInputLost);
    cross_a_to_b(&mut desk, 540);
    let out = desk.physical(A, key(Key::LEFT_CTRL, false));
    assert!(out.is_quiet(), "{out:?}");
    // Ctrl is no longer remembered as held, so Alt+End is typed, not a chord.
    desk.physical(A, key(Key::LEFT_ALT, true));
    let out = desk.physical(A, key(Key::END, true));
    assert!(!sends(out.of(A), PeerMessage::SecureAttention), "{out:?}");
}
