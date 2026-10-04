# DeskHop protocols

Two versioned protocols. A change to either bumps its version constant and updates this file in the same PR.

| Protocol | Between | Version constant | Current |
|---|---|---|---|
| Peer wire protocol | DeskHop peers on the LAN, over QUIC | `proto::PROTOCOL_VERSION` | 1 |
| IPC protocol | Service, agents, and UI, over named pipes | `ipc::IPC_VERSION` | 1 |

This document is written so that a third party could implement a compatible peer (ADR 0003, ADR 0004).

## Peer wire protocol

Shape: ADR 0004. Symmetric peers, mDNS/DNS-SD discovery with pair-by-code fallback, PAKE pairing over QUIC whose TLS is pinned to each peer's identity key, and QUIC with datagrams for motion and streams for everything else. Normative requirements: `openspec/specs/wire-protocol/spec.md`. All integers are little-endian.

### Stream frames

Every message on a QUIC stream is one frame:

```
+----------------+--------------+-------------------+
| len: u32le     | type: u16le  | payload: len bytes|
+----------------+--------------+-------------------+
  <------- 6-byte header ------->
```

- `len` counts the payload only. The header never changes in any version: it is read before a version is agreed.
- `len` must not exceed 65,536. A receiver rejects a larger value from the header alone, before the payload arrives. Messages with more data than that are split into several frames by the capability that defines them.
- Example: type `0x0001` with payload `01 00 01 00` is `04 00 00 00 01 00 01 00 01 00`.

### Datagrams

Every message sent as a QUIC datagram is one datagram, with no length field:

```
+--------------+---------------------------+
| type: u16le  | payload: rest of datagram |
+--------------+---------------------------+
```

A datagram is 2 to 1,200 bytes, type included. Anything shorter or longer is a protocol error.

### Message types

Stream and datagram messages share one 16-bit namespace. A type not defined in the negotiated version, or sent on a channel it may not use, is a protocol error. Every payload must match its layout exactly; leftover bytes are a protocol error, except for `Hello`.

| Type | Name | Channel | Since | Payload |
|---|---|---|---|---|
| `0x0000` | reserved | — | — | Never valid. |
| `0x0001` | `Hello` | stream (control) | 1 | See below. |
| `0x0002` | `PairPake` | stream | 1 | See [Pairing](#pairing). |
| `0x0003` | `PairConfirm` | stream | 1 | See [Pairing](#pairing). |
| `0x0004` | `MemberRecord` | stream | 1 | See [Desk membership](#desk-membership). |
| `0x0005` | `RecordsDone` | stream | 1 | Empty. See [Desk membership](#desk-membership). |
| `0x0006` | `FocusEnter` | stream | 1 | See [Engine messages](#engine-messages). |
| `0x0007` | `FocusRefused` | stream | 1 | Empty. |
| `0x0008` | `EdgeExit` | stream | 1 | See [Engine messages](#engine-messages). |
| `0x0009` | `TakenBack` | stream | 1 | Empty. |
| `0x000A` | `FocusWithdrawn` | stream | 1 | Empty. |
| `0x000B` | `ForgetSide` | stream | 1 | See [Engine messages](#engine-messages). |
| `0x000C` | `KeyInput` | stream | 1 | See [Engine messages](#engine-messages). |
| `0x000D` | `ButtonInput` | stream | 1 | See [Engine messages](#engine-messages). |
| `0x000E` | `WheelInput` | stream | 1 | See [Engine messages](#engine-messages). |
| `0x000F` | `SecureAttention` | stream | 1 | Empty. |
| `0x0010` | `Motion` | datagram | 1 | See [Motion](#motion). |

### Hello and version negotiation

```
Hello payload:
+-------------+-------------+-------------------------------------+
| min: u16le  | max: u16le  | fields from later versions (ignored) |
+-------------+-------------+-------------------------------------+
```

- As soon as the connection is up, each peer sends `Hello` as the first frame on the control stream, without waiting for the other's. `min` and `max` are the lowest and highest versions the sender speaks; `min` ≥ 1 and `min` ≤ `max`.
- Both peers then use `min(local.max, remote.max)` if it is at least `max(local.min, remote.min)`. Both sides compute the same result; there is no acknowledgement.
- If the ranges do not overlap, each peer closes with code `2`. The side whose `max` is below the other's `min` runs the older release.
- Protocol errors: a first control-stream frame that is not `Hello`, a second `Hello`, `Hello` on any other stream, `min` = 0, or `min` > `max`.
- Before the version is agreed, datagrams are dropped, and frames on other streams are held unread until agreement (QUIC does not order data across streams).
- `Hello` ignores bytes after `max`, so later versions can append fields. The first four bytes never change.

### Transport

Normative requirements: `openspec/specs/peer-transport/spec.md` and `openspec/specs/discovery/spec.md`.

**QUIC over UDP port 47391.** Every machine listens there, and dials peers on 47391 unless an mDNS advertisement names another port. A single UDP socket both listens and dials, so a peer's source address is also where it can be dialed back.

**TLS 1.3 only, signature scheme Ed25519 only.**

- *Certificates:* each side presents a self-signed X.509 certificate, in both directions, whose SubjectPublicKeyInfo is exactly the 44-byte RFC 8410 Ed25519 encoding `30 2a 30 05 06 03 2b 65 70 03 21 00` followed by the 32-byte key. That key is the machine's peer identity. Names, validity dates, extensions, and the issuer are ignored. The handshake signature proves possession of the key.
- *Pinning:* a dialer reaching a member accepts only that member's key. A dialer that is pairing accepts any key and uses it as the inviter's identity. A listener accepts any Ed25519 key during the handshake and decides afterwards.
- *Server name:* dialers send `deskhop`, and it is ignored.

**ALPN.**

| ALPN | Connection | After `Hello` |
|---|---|---|
| `deskhop/1` | Member | The record exchange below |
| `deskhop-pair/1` | Pairing | The pairing exchange ([Pairing](#pairing)) |

Any other ALPN fails the handshake.

**Control stream.** The dialer opens the connection's first bidirectional stream. Both sides write `Hello` on it at once ([Hello and version negotiation](#hello-and-version-negotiation)). Every stream message in this document travels on it; `Motion` travels in datagrams. A side finishes the stream only just before closing the connection.

**Member connections.**

1. The dialer sends every winning `MemberRecord` it holds, then `RecordsDone`.
2. The listener applies them. If the dialer's key is still not a member of the listener's view, the listener closes with code `5` and sends no records. Otherwise it sends its own records, then `RecordsDone`.
3. Each side counts the other as up once both `RecordsDone` messages have crossed.
4. After that, a `MemberRecord` is a single update, sent whenever a side's view gains or changes a record.

A removed member's connection receives the removal, then a close with code `5`.

**One connection per pair.** When two member connections between the same two machines are both up, both sides keep the one dialed by the machine whose peer identity is lower (comparing the 32 bytes in order), and close the other with code `0`.

**Liveness.**

- A side sends a keepalive after 250 ms without sending anything else. The QUIC idle timeout is 1 s, so a peer silent for 1 s is lost.
- Close code `0` on a member connection is a goodbye: the peer is down at once.

**Strangers.** A connection from a key that is not a member must send `PairPake` (pairing) or finish introducing itself (member) within 10 s. At most 8 such connections are held at once, and more are closed at once: with `4` on pairing connections and `5` on member connections.

**mDNS (DNS-SD).** Each machine advertises service type `_deskhop._udp.local.`:

| Field | Value |
|---|---|
| Instance name | First 16 bytes of the peer identity, lowercase hex (32 digits) |
| Port | The QUIC port |
| TXT `id` | The full peer identity, lowercase hex (64 digits) |
| TXT `pair` | `1` while pairing mode is open; absent otherwise |

An advertisement is a hint and is never trusted by itself: dialing pins the identity it names.

**Joining by code.** The joiner dials these candidates at once, on port 47391 or the advertised port:

- the code's locator rebuilt onto each of its own IPv4 interfaces;
- every `pair=1` advertisement whose IPv4 address ends in the locator.

The first candidate to answer `PairPake` with its own carries on, and the rest are closed with `0`.

### Identity

Each machine has one Ed25519 key. Its 32-byte public key is the machine's peer identity, and its TLS certificate carries that key ([Transport](#transport)). Normative requirements: `openspec/specs/pairing/spec.md`.

### Pairing

The *inviter* shows a code; on the *joiner*, the user types it. The code authenticates a SPAKE2 exchange, and key-confirmation tags bind the result to the connection.

**Code.** `<locator>-<secret><check>`, all digits, for example `137-4829153`.

- *Locator:* the trailing `4 - min(⌊prefix / 8⌋, 3)` bytes of the inviter's IPv4 address, in decimal, separated by `.` (1 byte on a /24, 2 on a /16, 3 on a /8). The joiner rebuilds the address from its own leading bytes.
- *Secret:* 6 digits, uniform over `000000`–`999999`.
- *Check:* one Damm check digit over the locator's decimal digits followed by the secret's digits.
- *Canonical text:* the form shown above, with no spaces. When parsing typed text, whitespace anywhere and hyphens after the first are ignored.

**SPAKE2.** Symmetric mode over the Ed25519 group, compatible with python-spake2's `SPAKE2_Symmetric` and the `spake2` Rust crate:

- *Password* `pw`: the UTF-8 bytes of the code's canonical text. *Identity* `id`: the ASCII bytes `deskhop pair v1`.
- *Password scalar:* HKDF-SHA256 with an empty salt, `pw` as input key material, and info `SPAKE2 pw`, expanded to 48 bytes. Read those bytes as a big-endian integer and reduce it modulo the group order.
- *Constant S:* the point whose compressed encoding is `6f00dae87c1be1a73b5922ef431cd8f57879569c222d22b1cd71e8546ab8e6f1`.
- *Message:* `0x53` (`S`), then the compressed point `x·B + pw_scalar·S`, for a random scalar `x`. 33 bytes.
- *Shared point:* `K = x·(Y − pw_scalar·S)`, where `Y` is the other side's point.
- *Shared key:* `SHA-256(SHA-256(pw) ‖ SHA-256(id) ‖ first ‖ second ‖ K)`, where `first` and `second` are the two 32-byte points in ascending byte order.

**Key confirmation.**

```
PRK     = HKDF-Extract(salt = "deskhop pair v1", IKM = shared key)        (SHA-256)
binding = exporter ‖ inviter certificate key ‖ joiner certificate key     (32 + 32 + 32 bytes)
tag_J   = HKDF-Expand(PRK, "joiner"  ‖ binding, 32)
tag_I   = HKDF-Expand(PRK, "inviter" ‖ binding, 32)
```

`exporter` is the TLS exporter value with label `EXPORTER-deskhop-pair-v1`, an empty context, and a length of 32. Tags are compared in constant time.

**Order**, after `Hello`:

1. The joiner sends `PairPake`. The inviter replies with `PairPake`, or closes with code `4` if pairing mode is closed or another attempt is running.
2. The joiner sends `PairConfirm` with `tag_J` and its name.
3. If `tag_J` is wrong, the inviter closes with code `3` and sends no tag. Otherwise it replies with `PairConfirm` carrying `tag_I` and its name.
4. If `tag_I` is wrong, the joiner closes with code `3`.
5. Each side signs a `MemberRecord` adding the other, then sends all its records and `RecordsDone`, as described under [Desk membership](#desk-membership). After receiving the inviter's `RecordsDone`, the joiner closes with code `0`.

Any other order is a protocol error.

**Limits on the inviter.**

- Pairing mode lasts 10 minutes, ends after one successful pairing, and handles one attempt at a time.
- An attempt starts at the joiner's `PairPake` and fails if any of these happens: a wrong tag, a protocol error, the connection dropping, or 30 seconds passing before the record exchange ends. A timed-out attempt is closed with code `1`.
- After 3 failed attempts, pairing mode closes and its code stops working. The user opens it again for a new code.

```
PairPake payload (33 bytes):
+-----------+---------------------+
| side: u8  | point: 32 bytes     |
| 0x53 'S'  | compressed Edwards  |
+-----------+---------------------+

PairConfirm payload (33 + n bytes):
+-----------------+-----------+---------------------+
| tag: 32 bytes   | n: u8     | name: n bytes UTF-8 |
|                 | 1..=63    |                     |
+-----------------+-----------+---------------------+
```

### Desk membership

Members sign records that add or remove a machine. A record is accepted when its Ed25519 signature verifies and its signer is a member in the receiver's view. Normative requirements: `openspec/specs/desk-membership/spec.md`.

```
MemberRecord payload (138 + n bytes):
+---------+------------------+------------+------------------+---------+------------+-------------------+
| kind:u8 | subject: 32 B    | epoch: u64 | signer: 32 B     | n: u8   | name: n B  | signature: 64 B   |
| 1 add   | peer identity    |            | peer identity    | add:    | UTF-8      | Ed25519 by signer |
| 2 remove|                  |            |                  | 1..=63  |            |                   |
|         |                  |            |                  | remove: |            |                   |
|         |                  |            |                  | 0       |            |                   |
+---------+------------------+------------+------------------+---------+------------+-------------------+
```

- *Signature:* covers the ASCII bytes `deskhop member record v1`, followed by the payload without its last 64 bytes. An addition whose signer is its own subject is invalid.
- *Which record wins:* for each subject, the record with the highest epoch wins; on equal epochs, removal wins. A machine holds records for at most 256 subjects.
- *Epoch:* the signer's Unix time in seconds, or one above the highest epoch it holds for that subject if that is greater, and at least 1.
- *Exchange:* on every connection between members, and at the end of pairing, each side sends every winning record it holds, then `RecordsDone` (an empty payload). A receiver applies the records when `RecordsDone` arrives, retrying records whose signer becomes a member partway through.
  - A `MemberRecord` after `RecordsDone` is a single update and is applied at once.
  - More than 256 records before `RecordsDone`, or a second `RecordsDone`, is a protocol error.
- *Removal of the receiver:* a machine that accepts a winning removal of itself drops every record. Its identity key is kept.

### Engine messages

What one machine's focus engine tells another's: focus changes, layout corrections, and forwarded keys, buttons and wheel. Normative requirements: `openspec/specs/input-forwarding/spec.md`.

- *Where:* only on a member connection whose peer is up (both `RecordsDone` exchanged), on the control stream, in the order sent. An engine message on a pairing connection, or on a member connection before the peer is up, is a protocol error.
- *Codes:* side `0` left, `1` right, `2` top, `3` bottom. Button `0` left, `1` right, `2` middle, `3` X1, `4` X2. Down `0` released, `1` pressed. Any other value is a protocol error.
- *Fraction:* a position along a side, `0` at the top or left end and `65535` at the other, across the whole extent of the machine's screens on that axis.
- *Wheel:* in 1/120 of a notch. Positive `dy` is away from the user, positive `dx` to the right.

| Message | Payload | Bytes |
|---|---|---|
| `FocusEnter` | side: u8, fraction: u16, from: 32 B peer identity | 35 |
| `EdgeExit` | target: 32 B peer identity, side: u8, fraction: u16 | 35 |
| `ForgetSide` | side: u8 | 1 |
| `KeyInput` | mark: 12 B, usage: u16 (USB HID keyboard page), down: u8 | 15 |
| `ButtonInput` | mark: 12 B, button: u8, down: u8 | 14 |
| `WheelInput` | mark: 12 B, dx: i32, dy: i32 | 20 |
| `FocusRefused`, `TakenBack`, `FocusWithdrawn`, `SecureAttention` | empty | 0 |

What each message means:

- `FocusEnter`: focus moves to the receiver through its `side` at `fraction`. `from` is the machine whose edge the cursor left.
- `FocusRefused`: another peer already controls the receiver of the `FocusEnter`, and the sender of the `FocusEnter` keeps focus.
- `EdgeExit`: the cursor left the controlled sender's `side` at `fraction`, toward `target`.
- `TakenBack`: the controlled sender's own user took focus back.
- `FocusWithdrawn`: the sender no longer forwards input to the receiver.
- `ForgetSide`: the receiver removes its slots on `side` that map to the sender.
- `SecureAttention`: the receiver shows its secure attention screen.

The mark is described under [Motion](#motion).

### Motion

Forwarded mouse movement travels as running totals in datagrams. A lost datagram loses no movement, because the next one carries it.

```
Motion payload, and the mark (12 bytes):
+-----------+---------------+---------------+
| seq: u32  | total_dx: i32 | total_dy: i32 |
+-----------+---------------+---------------+
```

- *Totals:* the sum of all movement sent on this connection since the peer came up, wrapping on overflow. The sender's first sequence number is 1. The receiver starts at sequence 0 with totals 0.
- *Applying:* a receiver accepts a mark only if `(seq − last_seq) mod 2³²`, read as a signed 32-bit integer, is positive. It reports the movement `total − last_total`, computed with wrapping arithmetic, and stores the mark. Older and equal marks are ignored.
- *Coalescing:* a connection sends at most one `Motion` datagram every 4 ms. Movement that arrives sooner goes in the next datagram, sent 4 ms after the previous one.
- *Repeat:* 20 ms after a `Motion` datagram, if no movement has arrived since, the same sequence number and totals are sent once more.
- *The mark:* every `KeyInput`, `ButtonInput` and `WheelInput` starts with a mark covering all movement forwarded before it, including movement not yet sent in a datagram. That movement then gets no datagram of its own. The receiver applies the mark before acting on the message, so a click never overtakes the movement before it.
- *Before up:* `Motion` datagrams that arrive before the peer is up are dropped, not treated as errors.

### Close reasons

Sent as the QUIC application error code when closing a connection.

| Code | Reason |
|---|---|
| `0` | Normal close |
| `1` | Protocol error |
| `2` | Version mismatch |
| `3` | Wrong pairing code: a key-confirmation tag did not match |
| `4` | Not ready to pair: pairing mode is closed, or another attempt is running |
| `5` | Not a member: the other machine does not count this one as a member of its desk |

## IPC protocol

Shape: ADR 0002. Service, agents, and UI may run different versions during an upgrade.

No messages are defined yet.

## Version history

| Protocol | Version | Change |
|---|---|---|
| Peer | 1 | Stream frames, datagrams, message-type registry, `Hello` version negotiation, close reasons, pairing (`PairPake`, `PairConfirm`), desk membership (`MemberRecord`, `RecordsDone`), transport (port 47391, identity certificates, ALPNs, control stream, member connections, one connection per pair, liveness, mDNS), close reason `5`, engine messages (`0x0006`–`0x000F`) and `Motion` datagrams (`0x0010`). Version 1 has not shipped, so these are added without a version bump. |
| IPC | 1 | Initial version; no messages. |
