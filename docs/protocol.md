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
| `0x0011` | `ClipboardOffer` | stream (control) | 1 | See [Clipboard](#clipboard). |
| `0x0012` | `ClipboardRequest` | stream (control) | 1 | See [Clipboard](#clipboard). |
| `0x0013` | `ClipboardData` | clipboard stream | 1 | See [Clipboard](#clipboard). |
| `0x0014` | `ClipboardChunk` | clipboard stream | 1 | See [Clipboard](#clipboard). |

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

**Control stream.** The dialer opens the connection's first bidirectional stream. Both sides write `Hello` on it at once ([Hello and version negotiation](#hello-and-version-negotiation)). Every stream message in this document travels on it, except `ClipboardData` and `ClipboardChunk`, which travel on clipboard streams ([Clipboard](#clipboard)); `Motion` travels in datagrams. A side finishes the stream only just before closing the connection. It is the only bidirectional stream: each side allows its peer one.

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

### Clipboard

Copied text follows the cursor: a copy is announced without its text, and a machine fetches it when the cursor crosses into it. Normative requirements: `openspec/specs/clipboard/spec.md`.

| Message | Where | Payload | Bytes |
|---|---|---|---|
| `ClipboardOffer` | control stream | id: u64 | 8 |
| `ClipboardRequest` | control stream | id: u64 | 8 |
| `ClipboardData` | clipboard stream, first frame | id: u64, len: u32 | 12 |
| `ClipboardChunk` | clipboard stream, after `ClipboardData` | 1 to 65,536 bytes of text | 1–65,536 |

- *Where:* only on a member connection whose peer is up, as for [engine messages](#engine-messages). On a pairing connection, before the peer is up, or on the wrong kind of stream, any of them is a protocol error.
- `ClipboardOffer`: the sender has a new shareable copy with this ID, chosen at random. It never carries text.
- `ClipboardRequest`: the sender asks for the receiver's copy with this ID. The receiver answers with a clipboard stream if that is the copy it holds, and otherwise with nothing.
- *Clipboard streams:* the sender of the text opens a unidirectional stream for one transfer. It writes one `ClipboardData`, then `ClipboardChunk` frames whose bytes total exactly `len`, then finishes the stream. Each side allows its peer one open clipboard stream at a time; QUIC flow control holds the next until the first ends.
- *Limits:* `len` is at most 4,194,304 and is checked before any chunk is read. A chunk outside that order, an empty chunk, bytes beyond `len`, a stream that ends short, or text that is not UTF-8 is a protocol error.
- *Text:* UTF-8, exactly as copied; line endings are not changed.
- *Unrequested:* a well-formed clipboard stream for an ID the receiver did not request, or no longer wants, is read and discarded.

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

Shape: ADR 0002. Service, agents, and UI may run different versions during an upgrade. Normative requirements: `openspec/specs/ipc-protocol/spec.md`, `openspec/specs/agent-link/spec.md` and `openspec/specs/ui-link/spec.md`. All integers are little-endian.

### IPC frames

```
+----------------+--------------+-------------------+
| len: u32le     | type: u16le  | payload: len bytes|
+----------------+--------------+-------------------+
```

- The same shape as peer stream frames, but with its own registry and version. `len` counts the payload only, and must not exceed 65,536. A larger value is rejected from the header alone.
- Pipes carry no close codes. An end that finds an error closes the pipe, and the service logs why.

### Hello and roles

```
Hello payload (type 0x0001):
+-------------+-------------+----------+--------------------------------------+
| min: u16le  | max: u16le  | role: u8 | fields from later versions (ignored) |
+-------------+-------------+----------+--------------------------------------+
```

- Each end sends `Hello` as its first frame at once, without waiting. Both then use the highest version in both ranges, and close the pipe if there is none.
- *Roles:* `1` session agent, `2` winlogon agent, `3` UI, `4` service. The service end of an agent link accepts only the role of the agent it launched on that pipe (`1` or `2`), the service end of a UI link only `3`, and agents and UIs accept only `4`.
- *Per role:* `ClipboardText`, `ClipboardChunk` and `ClipboardNotShared` may come only from a session agent, and `InputDesktop` only from a winlogon agent. Either from the other role is a protocol error. The service never sends `SetClipboard` to a winlogon agent.
- *Protocol errors:* a first frame that is not `Hello`, a second `Hello`, `min` = 0, `min` > `max`, or an undefined role.

### IPC message types

A type not defined in the agreed version, or travelling in a direction its type does not allow, is a protocol error. Directions name the link as well: a UI type on an agent link, or an agent type on a UI link, is travelling the wrong way. Every payload must match its layout exactly, except `Hello`'s trailing bytes.

| Type | Name | Direction | Since | Payload | Bytes |
|---|---|---|---|---|---|
| `0x0000` | reserved | — | — | Never valid. | — |
| `0x0001` | `Hello` | both | 1 | See above. | ≥ 5 |
| `0x0010` | `KeyEvent` | agent → service | 1 | at: u64, origin: u8, usage: u16, down: u8 | 12 |
| `0x0011` | `ButtonEvent` | agent → service | 1 | at: u64, origin: u8, button: u8, down: u8 | 11 |
| `0x0012` | `WheelEvent` | agent → service | 1 | at: u64, origin: u8, dx: i32, dy: i32 | 17 |
| `0x0013` | `MotionEvent` | agent → service | 1 | at: u64, origin: u8, dx: i32, dy: i32, x: i32, y: i32 | 25 |
| `0x0014` | `Screen` | agent → service | 1 | n: u8, then n monitors (below) | 1 + Σ(22 + id_len) |
| `0x0015` | `Pong` | agent → service | 1 | service_time: u64, agent_time: u64 | 16 |
| `0x0016` | `ClipboardText` | session agent → service | 1 | len: u32 (≤ 4,194,304) | 4 |
| `0x0017` | `ClipboardChunk` | session agent → service | 1 | text bytes | 1–65,536 |
| `0x0018` | `ClipboardNotShared` | session agent → service | 1 | reason: u8 | 1 |
| `0x0019` | `InputDesktop` | winlogon agent → service | 1 | active: u8 | 1 |
| `0x0020` | `SetCapture` | service → agent | 1 | mode: u8 | 1 |
| `0x0021` | `InjectKey` | service → agent | 1 | usage: u16, down: u8 | 3 |
| `0x0022` | `InjectButton` | service → agent | 1 | button: u8, down: u8 | 2 |
| `0x0023` | `InjectWheel` | service → agent | 1 | dx: i32, dy: i32 | 8 |
| `0x0024` | `InjectMotion` | service → agent | 1 | dx: i32, dy: i32 | 8 |
| `0x0025` | `Warp` | service → agent | 1 | x: i32, y: i32 | 8 |
| `0x0026` | `Ping` | service → agent | 1 | service_time: u64 | 8 |
| `0x0027` | `SetClipboard` | service → session agent | 1 | len: u32 (≤ 4,194,304) | 4 |
| `0x0028` | `SetClipboardChunk` | service → session agent | 1 | text bytes | 1–65,536 |
| `0x0030` | `OpenPairing` | UI → service | 1 | — | 0 |
| `0x0031` | `ClosePairing` | UI → service | 1 | — | 0 |
| `0x0032` | `Join` | UI → service | 1 | code | 2–33 |
| `0x0033` | `RemovePeer` | UI → service | 1 | key | 32 |
| `0x0034` | `Undo` | UI → service | 1 | — | 0 |
| `0x0035` | `ForgetLayout` | UI → service | 1 | — | 0 |
| `0x0038` | `Status` | service → UI | 1 | See "UI link". | ≥ 39 |
| `0x0039` | `Paired` | service → UI | 1 | key, name | 34–96 |
| `0x003A` | `JoinFailed` | service → UI | 1 | reason: u8 | 1 |
| `0x003B` | `PairingClosed` | service → UI | 1 | reason: u8 | 1 |
| `0x003C` | `PairingNotOpened` | service → UI | 1 | reason: u8 | 1 |
| `0x003D` | `Learned` | service → UI | 1 | key, side: u8, undo_ms: u32 | 37 |

**Codes:** origin `0` physical, `1` injected. Down `0` released, `1` pressed. Button `0` left, `1` right, `2` middle, `3` X1, `4` X2. Mode `0` pass all, `1` withhold all, `2` withhold mouse. Clipboard reason `0` private, `1` no text, `2` text too large. Active `0` another desktop has the input, `1` the agent's own desktop is the input desktop. Any other value is a protocol error.

**Clipboard runs:** text travels as a run. A `ClipboardText` or `SetClipboard` gives the length in bytes of UTF-8, and the chunks of the same direction that follow carry exactly that many bytes; a length of 0 is a complete run with no chunks. Other messages, input included, may come between chunks. A chunk outside a run, a chunk with no bytes, more bytes than announced, a new run or `ClipboardNotShared` before the run completes, or text that is not UTF-8 is a protocol error. The agent reports a copy once its clipboard has been still for 100 ms. It reports a copy marked to be kept out of clipboard monitoring, history or cloud sync as `ClipboardNotShared` with reason `0`, without reading its text, and never reports its own `SetClipboard` back. Normative requirements: `openspec/specs/agent-link/spec.md` and `openspec/specs/clipboard/spec.md`.

**Fields:**
- *usage:* a USB HID keyboard-page usage.
- *dx, dy (motion):* raw device movement.
- *x, y:* the cursor in virtual-screen physical pixels.
- *Wheel:* in 1/120 of a notch.
- *at:* milliseconds on the agent's monotonic clock (the performance counter).

**`Screen` monitors:**

```
+-------------+-------------+--------+--------+--------+--------+----------+
| id_len: u16 | id: UTF-8   | x: i32 | y: i32 | w: i32 | h: i32 | dpi: u32 |
+-------------+-------------+--------+--------+--------+--------+----------+
```

Limits: at most 32 monitors; `id_len` from 1 to 256; `w` and `h` at least 1; `x + w` and `y + h` within `i32`; `dpi` from 48 to 960. A screen that breaks a limit is a protocol error.

### Clock alignment

- The service sends `Ping(t0)` with its own clock reading as soon as the link is up, and every 10 s after. The agent answers at once with `Pong(t0, a)`, where `a` is its own clock.
- On receiving the `Pong` at `t1`, the service computes `offset = a − (t0 + t1) / 2` and keeps the estimate from the exchange with the smallest `t1 − t0`.
- An agent time `at` becomes `at − offset` on the service's clock, clamped at 0.
- Until the first `Pong`, the service holds the agent's events in order (at most 10,000; more closes the link) and does not report them.

### Agent link behavior

- The service starts the session agent as `deskhop-agent.exe --session --pipe <name>` and the winlogon agent as `deskhop-agent.exe --winlogon --pipe <name>`. The agent connects, exchanges `Hello` with its role, and only then starts capturing input.
- The winlogon agent sends `InputDesktop` right after its first `Screen`, and again within 100 ms of each change. It never watches or sets the clipboard.
- The agent exits with code `3` if it cannot open the pipe, `4` if the handshake fails, and `5` if capture cannot start.
- When the link ends for any reason, the agent returns capture to pass all, removes its hooks, and exits.
- Sending a command never waits. If more than 4,096 commands are queued for an agent that is not reading, the service closes the link.

### UI link

The UI connects to `\\.\pipe\deskhop-ui`, which the service creates at start as the first instance of that name. The pipe refuses remote clients and serves at most 8 UIs at once. Its ACL admits SYSTEM and interactive users. In console mode it admits only the user running console mode.

**Fields:**
- *key:* a machine's 32-byte Ed25519 public key.
- *name:* `name_len: u8` from 1 to 63, then that many bytes of UTF-8.
- *code:* `code_len: u8` from 1 to 32, then that many bytes of printable ASCII (`0x21`–`0x7E`). A `Join` carries the code as the user typed it; the service parses it.

**`Status`:**

Fields in order:

```
key, name                     this machine
agent: u8
pairing: u8
  code, left_ms: u32          only when pairing = 1
focus: u8
  key                         only when focus ≠ 0
n: u16
  n × (key, name, up: u8)     the members
```

- *agent:* `0` no agent linked, `1` linked.
- *pairing:* `0` closed, `1` open; `left_ms` is the time pairing stays open.
- *focus:* `0` local, `1` this machine's input is on the member `key`, `2` the member `key` controls this machine.
- *members:* at most 256; `up` is `0` or `1`.

**Codes:**
- `JoinFailed`: `0` wrong code, `1` not ready, `2` protocol error, `3` desk full, `4` connection lost, `5` unreachable, `6` mistyped code (its check digit is wrong), `7` malformed code.
- `PairingClosed`: `0` a machine joined, `1` time ran out, `2` closed by the user, `3` too many wrong codes.
- `PairingNotOpened`: `0` no IPv4 network.
- *side* (`Learned`): `0` left, `1` right, `2` top, `3` bottom.

Any other value, a name or code outside its limits, or more than 256 members is a protocol error.

**Behavior:**
- **Who is accepted:** the service accepts a UI only if the connecting process runs in the active console session and sends `Hello` with role `3`. Any other client is closed before it is sent anything but `Hello`.
- **Status:** after `Hello`, the service sends `Status` at once, and again whenever a field changes, at most once every 100 ms and always with the latest values. Each `Status` is complete.
- **Notices:** `Paired`, `JoinFailed`, `PairingClosed`, `PairingNotOpened` and `Learned` go to every connected UI when they happen.
- **Commands:** they act as the console commands do. A `RemovePeer` for a key that is not a member changes nothing.
- **Never waits:** sending to a UI never waits. A UI with more than 256 unsent messages is disconnected.

## Version history

| Protocol | Version | Change |
|---|---|---|
| Peer | 1 | Stream frames, datagrams, message-type registry, `Hello` version negotiation, close reasons, pairing (`PairPake`, `PairConfirm`), desk membership (`MemberRecord`, `RecordsDone`), transport (port 47391, identity certificates, ALPNs, control stream, member connections, one connection per pair, liveness, mDNS), close reason `5`, engine messages (`0x0006`–`0x000F`), `Motion` datagrams (`0x0010`), and the clipboard (`0x0011`–`0x0014`, clipboard streams). Version 1 has not shipped, so these are added without a version bump. |
| IPC | 1 | Frames, `Hello` with roles and version negotiation, the message registry with directions, and the agent link (input and screen reports, `SetCapture`, injection, warp, `Ping`/`Pong` clock alignment), and clipboard runs (`0x0016`–`0x0018`, `0x0027`–`0x0028`), and the winlogon agent (`InputDesktop`, `0x0019`, and the messages allowed per agent role). The UI link (`0x0030`–`0x003D`: pairing, join, remove, undo, forget layout, `Status` and notices), with directions per link. Version 1 has not shipped, so these fill it in without a version bump. |
