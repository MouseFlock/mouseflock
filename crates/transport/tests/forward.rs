//! Engine messages and motion between nodes on loopback (spec:
//! input-forwarding).

mod harness;

use std::time::Duration;

use harness::*;
use model::{Button, EdgeFraction, InputAction, Key, PeerId, PeerMessage, Side};
use pairing::{Identity, Machine, MachineName};
use proto::hello::Hello;
use proto::input::{ButtonInput, FocusEnter, InputMessage, KeyInput, Mark};
use transport::tls::{ALPN_MEMBER, ALPN_PAIR, Expect};
use transport::{Event, records_done_frame};

const WAIT: Duration = Duration::from_secs(5);

fn message_from(peer: PeerId) -> impl Fn(&Event) -> bool {
    move |e| matches!(e, Event::Message { peer: p, .. } if *p == peer)
}

/// The engine messages `node` reported from `peer`, until `last` matches
/// one; fails after `within`.
async fn messages_until(
    node: &mut Node,
    peer: PeerId,
    within: Duration,
    last: impl Fn(&PeerMessage) -> bool,
) -> Vec<PeerMessage> {
    let mut got = Vec::new();
    loop {
        let Event::Message { msg, .. } = node.expect(within, message_from(peer)).await else {
            unreachable!()
        };
        got.push(msg);
        if last(&msg) {
            return got;
        }
    }
}

fn key(usage: u16, down: bool) -> PeerMessage {
    PeerMessage::Input(InputAction::Key {
        key: Key(usage),
        down,
    })
}

fn motion(dx: i32, dy: i32) -> PeerMessage {
    PeerMessage::Input(InputAction::Motion { dx, dy })
}

/// Total movement among `messages`.
fn moved(messages: &[PeerMessage]) -> (i64, i64) {
    messages.iter().fold((0, 0), |(x, y), m| match m {
        PeerMessage::Input(InputAction::Motion { dx, dy }) => {
            (x + i64::from(*dx), y + i64::from(*dy))
        }
        _ => (x, y),
    })
}

// ---- 3.2 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn every_message_crosses() {
    let (a, mut b) = connected_pair().await;
    let sent = vec![
        PeerMessage::FocusEnter {
            side: Side::Left,
            fraction: EdgeFraction(0x4000),
            from: a.id,
        },
        PeerMessage::FocusRefused,
        PeerMessage::EdgeExit {
            target: PeerId([3; 32]),
            side: Side::Bottom,
            fraction: EdgeFraction(9),
        },
        PeerMessage::TakenBack,
        PeerMessage::FocusWithdrawn,
        PeerMessage::ForgetSide { side: Side::Top },
        key(0x04, true),
        PeerMessage::Input(InputAction::Button {
            button: Button::X1,
            down: true,
        }),
        PeerMessage::Input(InputAction::Wheel { dx: 0, dy: -120 }),
        PeerMessage::SecureAttention,
    ];
    for msg in &sent {
        a.handle.send(b.id, *msg);
    }
    let got = messages_until(&mut b, a.id, WAIT, |m| *m == PeerMessage::SecureAttention).await;
    assert_eq!(got, sent);

    // Motion crosses too, rebuilt from the totals.
    a.handle.send(b.id, motion(7, -3));
    let got = messages_until(&mut b, a.id, WAIT, |m| {
        matches!(m, PeerMessage::Input(InputAction::Motion { .. }))
    })
    .await;
    assert_eq!(got, [motion(7, -3)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_keeps_order() {
    let (a, mut b) = connected_pair().await;
    // h e l l o
    let usages = [0x0b, 0x08, 0x0f, 0x0f, 0x12];
    let mut sent = Vec::new();
    for u in usages {
        sent.push(key(u, true));
        sent.push(key(u, false));
    }
    for msg in &sent {
        a.handle.send(b.id, *msg);
    }
    let got = messages_until(&mut b, a.id, WAIT, |m| *m == key(0x12, false)).await;
    assert_eq!(got, sent);
}

#[tokio::test(flavor = "multi_thread")]
async fn send_to_a_peer_that_is_down() {
    let (a, mut b) = connected_pair().await;
    a.handle.send(PeerId([9; 32]), PeerMessage::TakenBack);
    a.handle.send(b.id, PeerMessage::TakenBack);
    let got = messages_until(&mut b, a.id, WAIT, |_| true).await;
    assert_eq!(got, [PeerMessage::TakenBack]);
}

// ---- 3.3 -----------------------------------------------------------------

/// A raw client that `a`'s machine counts as a member, and `a` started.
fn member_client(name: &str) -> (RawClient, Node) {
    let client = RawClient::new();
    let identity = Identity::from_file(&*client.identity.to_file()).unwrap();
    let mut theirs = Machine::new(identity, MachineName::new("Raw").unwrap());
    let mut mine = machine(name);
    befriend(&mut mine, &mut theirs);
    (client, node(mine))
}

fn frame(m: InputMessage) -> Vec<u8> {
    let mut out = Vec::new();
    m.encode(&mut out).unwrap();
    out
}

fn hello() -> Vec<u8> {
    let mut out = Vec::new();
    Hello::this_release().encode_frame(&mut out).unwrap();
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_message_before_records() {
    let (client, a) = member_client("A");
    let connection = client
        .connect(a.addr, Expect::Peer(a.id), ALPN_MEMBER)
        .await;
    let (mut send, _recv) = connection.open_bi().await.unwrap();
    let mut out = hello();
    out.extend(frame(InputMessage::FocusEnter(FocusEnter {
        side: proto::input::Side::Left,
        fraction: 0,
        from: [0; 32],
    })));
    send.write_all(&out).await.unwrap();
    assert_eq!(close_code(&connection, WAIT).await, Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_message_on_a_pairing_connection() {
    let a = node(machine("A"));
    a.handle.open_pairing().await.unwrap();
    let client = RawClient::new();
    let connection = client.connect(a.addr, Expect::Any, ALPN_PAIR).await;
    let (mut send, _recv) = connection.open_bi().await.unwrap();
    let mut out = hello();
    out.extend(frame(InputMessage::Key(KeyInput {
        mark: Mark::default(),
        usage: 0x04,
        down: true,
    })));
    send.write_all(&out).await.unwrap();
    assert_eq!(close_code(&connection, WAIT).await, Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_button() {
    let (client, mut a) = member_client("A");
    let connection = client
        .connect(a.addr, Expect::Peer(a.id), ALPN_MEMBER)
        .await;
    let (mut send, _recv) = connection.open_bi().await.unwrap();
    let mut out = hello();
    out.extend(records_done_frame());
    send.write_all(&out).await.unwrap();
    a.up(client.identity.peer_id()).await;

    // A valid button frame with its button code changed to 9.
    let mut bad = frame(InputMessage::Button(ButtonInput {
        mark: Mark::default(),
        button: proto::input::Button::Left,
        down: true,
    }));
    let at = bad.len() - 2;
    bad[at] = 9;
    send.write_all(&bad).await.unwrap();
    assert_eq!(close_code(&connection, WAIT).await, Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn motion_before_up_is_dropped() {
    let (client, mut a) = member_client("A");
    let connection = client
        .connect(a.addr, Expect::Peer(a.id), ALPN_MEMBER)
        .await;
    let (mut send, _recv) = connection.open_bi().await.unwrap();
    send.write_all(&hello()).await.unwrap();
    wait(100).await;
    connection
        .send_datagram(
            frame(InputMessage::Motion(Mark {
                seq: 1,
                dx: 50,
                dy: 0,
            }))
            .into(),
        )
        .unwrap();
    wait(100).await;
    send.write_all(&records_done_frame()).await.unwrap();

    let raw = client.identity.peer_id();
    a.up(raw).await;
    a.none_for(Duration::from_millis(300), |e| {
        matches!(e, Event::Message { .. } | Event::PeerDown(..))
    })
    .await;
    assert_eq!(
        close_code(&connection, Duration::from_millis(1)).await,
        None
    );
}

// ---- 3.4 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn click_lands_after_its_motion() {
    let (a, mut b) = connected_pair().await;
    let press = PeerMessage::Input(InputAction::Button {
        button: Button::Left,
        down: true,
    });
    for _ in 0..10 {
        a.handle.send(b.id, motion(3, 1));
    }
    a.handle.send(b.id, press);
    let got = messages_until(&mut b, a.id, WAIT, |m| *m == press).await;
    assert_eq!(moved(&got), (30, 10), "{got:?}");
    assert_eq!(got.last(), Some(&press));
}

#[tokio::test(flavor = "multi_thread")]
async fn motion_survives_loss() {
    let (mut a, mut b) = (machine("A"), machine("B"));
    befriend(&mut a, &mut b);
    let (mut a, mut b) = (node(a), node(b));
    let relay = Relay::start(b.addr).await;
    a.sees_at(b.id, relay.addr, false);
    let (ida, idb) = (a.id, b.id);
    a.up(idb).await;
    b.up(ida).await;
    relay.lossy();

    for i in 0..1_000 {
        a.handle.send(b.id, motion(1, 0));
        if i % 10 == 9 {
            wait(1).await;
        }
    }
    let enter = key(0x28, true);
    a.handle.send(b.id, enter);
    let got = messages_until(&mut b, a.id, Duration::from_secs(10), |m| *m == enter).await;
    assert_eq!(moved(&got), (1_000, 0));
}
