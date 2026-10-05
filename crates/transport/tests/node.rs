//! Nodes on loopback (specs: peer-transport, discovery).

mod harness;

use std::time::Duration;

use harness::*;
use pairing::{Code, JoinFailure, ModeClosed, UnixTime};
use proto::frame::{self, Decoded};
use proto::hello::Hello;
use proto::registry;
use tokio::time::Instant;
use transport::tls::{ALPN_MEMBER, ALPN_PAIR, Expect};
use transport::{Down, Event};

fn is_down(e: &Event) -> bool {
    matches!(e, Event::PeerDown(..))
}

// ---- 4.1 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn port_already_in_use() {
    let taken = std::net::UdpSocket::bind((LOCAL, 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    let mut config = config(machine("A"));
    config.port = port;
    let error = transport::start(config)
        .err()
        .expect("started on a taken port");
    assert!(error.to_string().contains(&port.to_string()), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_agrees_version_1() {
    let a = node(machine("A"));
    let client = RawClient::new();
    let connection = client
        .connect(a.addr, Expect::Peer(a.id), ALPN_MEMBER)
        .await;
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    let mut hello = Vec::new();
    Hello::this_release().encode_frame(&mut hello).unwrap();
    send.write_all(&hello).await.unwrap();
    let mut buf = vec![0u8; 10];
    recv.read_exact(&mut buf).await.unwrap();
    let Decoded::Frame { frame, .. } = frame::decode(&buf).unwrap() else {
        panic!("incomplete")
    };
    assert_eq!(frame.ty, registry::HELLO);
    assert_eq!(
        Hello::decode(frame.payload).unwrap(),
        Hello { min: 1, max: 1 }
    );
}

// ---- 4.2 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn members_connect() {
    let (a, b) = connected_pair().await;
    assert!(a.seen.contains(&Event::PeerUp(b.id)));
    assert!(b.seen.contains(&Event::PeerUp(a.id)));
}

#[tokio::test(flavor = "multi_thread")]
async fn introduced_by_records() {
    let (mut a, mut b, mut c) = (machine("A"), machine("B"), machine("C"));
    befriend(&mut a, &mut b);
    // C pairs with B while A is away: B signs C in, C learns B's records.
    b.add(c.peer_id(), c.name().clone(), UnixTime(2)).unwrap();
    c.add(b.peer_id(), b.name().clone(), UnixTime(2)).unwrap();
    c.apply(b.desk().records().cloned().collect::<Vec<_>>());
    assert!(c.desk().is_member(&a.peer_id()));
    assert!(!a.desk().is_member(&c.peer_id()));
    let (mut a, mut c) = (node(a), node(c));
    c.sees(&a, false);
    let (ida, idc) = (a.id, c.id);
    c.up(ida).await;
    a.up(idc).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stranger_on_a_member_connection() {
    let a = machine("A");
    let mut x = machine("X");
    // X believes A is in its desk; A has never heard of X.
    x.add(a.peer_id(), a.name().clone(), UnixTime(1)).unwrap();
    let (mut a, mut x) = (node(a), node(x));
    x.sees(&a, false);
    let ida = a.id;
    x.none_for(Duration::from_millis(1_500), |e| {
        matches!(e, Event::PeerUp(_) | Event::TrustStore(_))
    })
    .await;
    a.none_for(Duration::from_millis(100), |e| {
        matches!(e, Event::PeerUp(_))
    })
    .await;
    let _ = ida;
}

#[tokio::test(flavor = "multi_thread")]
async fn stranger_is_closed_with_not_a_member_and_sees_no_records() {
    let (a, _b) = connected_pair().await;
    let client = RawClient::new();
    let connection = client
        .connect(a.addr, Expect::Peer(a.id), ALPN_MEMBER)
        .await;
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    let mut out = Vec::new();
    Hello::this_release().encode_frame(&mut out).unwrap();
    out.extend(transport::records_done_frame());
    send.write_all(&out).await.unwrap();
    let mut received = Vec::new();
    let mut chunk = [0u8; 1024];
    while let Ok(Some(n)) = recv.read(&mut chunk).await {
        received.extend_from_slice(&chunk[..n]);
    }
    assert_eq!(close_code(&connection, SECOND).await, Some(5));
    // Only A's Hello arrived: no record.
    let Decoded::Frame { frame, consumed } = frame::decode(&received).unwrap() else {
        panic!("no Hello")
    };
    assert_eq!(frame.ty, registry::HELLO);
    assert_eq!(consumed, received.len(), "records were sent to a stranger");
}

// ---- 4.3 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn pairing_then_connecting() {
    let (mut a, mut b) = (node(machine("A")), node(machine("B")));
    pair(&mut b, &mut a).await;
    a.saw(SECOND, |e| *e == Event::PairingClosed(ModeClosed::Paired))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_mode_closed() {
    let (mut a, mut b) = (node(machine("A")), node(machine("B")));
    let code = a.handle.open_pairing().await.unwrap();
    a.handle.close_pairing();
    a.expect(SECOND, |e| *e == Event::PairingClosed(ModeClosed::ByUser))
        .await;
    b.sees(&a, true);
    b.handle.join(code);
    b.expect(Duration::from_secs(8), |e| {
        *e == Event::JoinFailed(JoinFailure::NotReady)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn inviters_mode_closed() {
    // One candidate refuses with 4; the other (the rebuilt address on port 9)
    // never answers. Not ready wins over unreachable.
    let (a, mut b) = (node(machine("A")), node(machine("B")));
    let code = Code::generate(LOCAL, 8, &mut rand_core::OsRng);
    b.sees(&a, true);
    b.handle.join(code);
    b.expect(Duration::from_secs(8), |e| {
        *e == Event::JoinFailed(JoinFailure::NotReady)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_answers() {
    let mut b = node(machine("B"));
    let code = Code::generate(LOCAL, 8, &mut rand_core::OsRng);
    b.handle.join(code);
    // Within the 5-second handshake deadline, or sooner when the OS reports
    // the port unreachable.
    b.expect(Duration::from_secs(7), |e| {
        *e == Event::JoinFailed(JoinFailure::Unreachable)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn multicast_blocked() {
    let inviter_ip = std::net::Ipv4Addr::new(127, 0, 0, 2);
    let mut ca = config(machine("A"));
    ca.bind = inviter_ip.into();
    ca.network = Some(network(inviter_ip));
    let mut a = start(ca);
    let mut cb = config(machine("B"));
    cb.network = Some(network(std::net::Ipv4Addr::new(127, 0, 0, 5)));
    cb.dial_port = a.addr.port();
    let mut b = start(cb);
    let code = a.handle.open_pairing().await.unwrap();
    assert_eq!(code.locator_text(), "0.0.2");
    b.handle.join(code);
    let (ida, idb) = (a.id, b.id);
    b.up(ida).await;
    a.up(idb).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_code_over_the_network() {
    let (mut a, mut b) = (node(machine("A")), node(machine("B")));
    let shown = a.handle.open_pairing().await.unwrap();
    let wrong = shown.with_new_secret(&mut rand_core::OsRng);
    b.sees(&a, true);
    b.handle.join(wrong);
    b.expect(Duration::from_secs(8), |e| {
        *e == Event::JoinFailed(JoinFailure::WrongCode)
    })
    .await;
    a.none_for(Duration::from_millis(200), |e| {
        matches!(e, Event::Paired { .. })
    })
    .await;
}

// ---- 4.4 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn both_dial_at_once() {
    for _ in 0..20 {
        let (mut ma, mut mb) = (machine("A"), machine("B"));
        befriend(&mut ma, &mut mb);
        let (mut a, mut b) = (node(ma), node(mb));
        a.sees(&b, false);
        b.sees(&a, false);
        let (ida, idb) = (a.id, b.id);
        a.up(idb).await;
        b.up(ida).await;
        let ups = |e: &Event| matches!(e, Event::PeerUp(_));
        let either = |e: &Event| ups(e) || is_down(e);
        a.none_for(Duration::from_millis(600), either).await;
        b.none_for(Duration::from_millis(50), either).await;
        // One connection on each side: the one dialed by the lower identity.
        let lower_is_a = a.id < b.id;
        let on_a = a.handle.connections().await;
        let on_b = b.handle.connections().await;
        assert_eq!(on_a.len(), 1);
        assert_eq!(on_b.len(), 1);
        assert_eq!(on_a[0].dialed_here, lower_is_a);
        assert_eq!(on_b[0].dialed_here, !lower_is_a);
        a.handle.shutdown();
        b.handle.shutdown();
    }
}

// ---- 4.5 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn peer_goes_silent() {
    let (mut ma, mut mb) = (machine("A"), machine("B"));
    befriend(&mut ma, &mut mb);
    let (mut a, mut b) = (node(ma), node(mb));
    let relay = Relay::start(b.addr).await;
    a.sees_at(b.id, relay.addr, false);
    let (ida, idb) = (a.id, b.id);
    a.up(idb).await;
    b.up(ida).await;
    wait(300).await;
    relay.cut();
    let cut = Instant::now();
    a.expect(Duration::from_secs(3), |e| {
        *e == Event::PeerDown(idb, Down::Lost)
    })
    .await;
    let elapsed = cut.elapsed();
    assert!(
        elapsed >= Duration::from_millis(750),
        "lost after only {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn goodbye_before_sleep() {
    let (mut a, mut b) = connected_pair().await;
    let idb = b.id;
    let ida = a.id;
    let said = Instant::now();
    b.handle.goodbye().await;
    a.expect(Duration::from_millis(200), |e| {
        *e == Event::PeerDown(idb, Down::Goodbye)
    })
    .await;
    assert!(said.elapsed() <= Duration::from_millis(200));
    b.expect(SECOND, |e| *e == Event::PeerDown(ida, Down::Goodbye))
        .await;
    // B stays away until it resumes.
    b.none_for(Duration::from_millis(1_500), |e| {
        matches!(e, Event::PeerUp(_))
    })
    .await;
    b.handle.resume();
    b.up(ida).await;
}

// ---- 4.6 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn peer_comes_back() {
    let (mut ma, mut mb) = (machine("A"), machine("B"));
    befriend(&mut ma, &mut mb);
    let (mut a, mut b) = (node(ma), node(mb));
    let relay = Relay::start(b.addr).await;
    a.sees_at(b.id, relay.addr, false);
    let (ida, idb) = (a.id, b.id);
    a.up(idb).await;
    b.up(ida).await;
    relay.cut();
    a.expect(Duration::from_secs(3), |e| {
        *e == Event::PeerDown(idb, Down::Lost)
    })
    .await;
    // Let the backoff grow past a second, then come back.
    wait(2_000).await;
    relay.restore();
    let back = Instant::now();
    a.sees_at(idb, relay.addr, false);
    a.up(idb).await;
    assert!(back.elapsed() <= SECOND, "took {:?}", back.elapsed());
}

// ---- 4.7 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn removal_reaches_the_removed_machine() {
    let (mut a, mut b) = connected_pair().await;
    let (ida, idb) = (a.id, b.id);
    a.handle.remove(idb);
    a.expect(SECOND, |e| *e == Event::PeerDown(idb, Down::Removed))
        .await;
    b.expect(SECOND, |e| *e == Event::PeerDown(ida, Down::Removed))
        .await;
    // B forgot the desk: its trust store holds no records.
    let Event::TrustStore(file) = b
        .saw(
            SECOND,
            |e| matches!(e, Event::TrustStore(f) if f.len() == 7),
        )
        .await
    else {
        unreachable!()
    };
    assert_eq!(&file[..5], b"DHTS\x01");
    // And A does not reconnect to it.
    a.none_for(Duration::from_millis(1_500), |e| *e == Event::PeerUp(idb))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn new_member_spreads() {
    let (mut a, mut b) = connected_pair().await;
    let mut c = node(machine("C"));
    // A can see C on the network, but C is a stranger to it so far.
    a.sees(&c, false);
    pair(&mut c, &mut b).await;
    let (ida, idc) = (a.id, c.id);
    // A may connect to C before C's member connection to B is up.
    a.saw(Duration::from_secs(5), |e| *e == Event::PeerUp(idc))
        .await;
    c.saw(Duration::from_secs(5), |e| *e == Event::PeerUp(ida))
        .await;
}

// ---- 4.8 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn idle_stranger() {
    let mut config = config(machine("A"));
    config.stranger_timeout = SECOND;
    let a = start(config);
    let client = RawClient::new();
    let connection = client.connect(a.addr, Expect::Any, ALPN_PAIR).await;
    let started = Instant::now();
    assert_eq!(
        close_code(&connection, Duration::from_secs(3)).await,
        Some(4)
    );
    assert!(started.elapsed() >= Duration::from_millis(800));
}

#[tokio::test(flavor = "multi_thread")]
async fn ninth_stranger() {
    let a = node(machine("A"));
    let clients: Vec<RawClient> = (0..9).map(|_| RawClient::new()).collect();
    let mut first = Vec::new();
    for client in &clients[..8] {
        first.push(client.connect(a.addr, Expect::Any, ALPN_PAIR).await);
    }
    wait(200).await;
    let ninth = clients[8].connect(a.addr, Expect::Any, ALPN_PAIR).await;
    assert_eq!(close_code(&ninth, SECOND).await, Some(4));
    for connection in &first {
        assert!(
            connection.close_reason().is_none(),
            "an earlier stranger was closed"
        );
    }
}

// ---- 4.9 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn addresses_reported_after_pairing() {
    // Both sides learn the other's address during pairing, so both must
    // report it for storage, the inviter included.
    let (mut joiner, mut inviter) = (node(machine("J")), node(machine("I")));
    pair(&mut joiner, &mut inviter).await;
    let (j, i) = (joiner.id, inviter.id);
    let (jaddr, iaddr) = (joiner.addr, inviter.addr);
    joiner
        .saw(
            SECOND,
            |e| matches!(e, Event::Addresses(a) if a.get(&i) == Some(&iaddr)),
        )
        .await;
    inviter
        .saw(
            SECOND,
            |e| matches!(e, Event::Addresses(a) if a.get(&j) == Some(&jaddr)),
        )
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_on_a_network_without_multicast() {
    let (mut ma, mut mb) = (machine("A"), machine("B"));
    befriend(&mut ma, &mut mb);
    let (ra, rb) = (reload(&ma), reload(&mb));
    let (mut a, mut b) = (node(ma), node(mb));
    a.sees(&b, false);
    let (ida, idb) = (a.id, b.id);
    let (pa, pb) = (a.addr.port(), b.addr.port());
    a.up(idb).await;
    b.up(ida).await;
    let Event::Addresses(addrs_a) = a.saw(SECOND, |e| matches!(e, Event::Addresses(_))).await
    else {
        unreachable!()
    };
    let Event::Addresses(addrs_b) = b.saw(SECOND, |e| matches!(e, Event::Addresses(_))).await
    else {
        unreachable!()
    };
    assert_eq!(addrs_a.get(&idb), Some(&b.addr));
    a.handle.shutdown();
    b.handle.shutdown();
    wait(300).await;

    // Restart from what the service stored: same identities, trust stores,
    // ports and addresses; no sightings.
    let mut ca = config(ra);
    ca.port = pa;
    ca.addresses = addrs_a;
    let mut cb = config(rb);
    cb.port = pb;
    cb.addresses = addrs_b;
    let (mut a, mut b) = (start(ca), start(cb));
    a.up(idb).await;
    b.up(ida).await;
}

// ---- 5.1 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn member_found() {
    let (mut ma, mut mb) = (machine("A"), machine("B"));
    befriend(&mut ma, &mut mb);
    let (mut a, b) = (node(ma), node(mb));
    a.none_for(Duration::from_millis(300), |e| {
        matches!(e, Event::PeerUp(_))
    })
    .await;
    a.sees(&b, false);
    a.up(b.id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn spoofed_advertisement() {
    let (mut ma, mut mb) = (machine("A"), machine("B"));
    befriend(&mut ma, &mut mb);
    let (mut a, mut x) = (node(ma), node(machine("X")));
    // X's address, advertised as B.
    a.sees_at(mb.peer_id(), x.addr, false);
    a.none_for(Duration::from_millis(1_500), |e| {
        matches!(e, Event::PeerUp(_))
    })
    .await;
    x.none_for(Duration::from_millis(100), |_| true).await;
}
