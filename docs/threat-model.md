# DeskHop threat model

*Status: covers the wire, pairing, desk membership, the LAN transport, forwarded input, and the agent → service pipe. The UI pipe and the secure desktop are filled in with their changes.*

DeskHop injects keystrokes received over the network, and its service runs as SYSTEM. This document records what it defends against and where.

## Assets

- Identity keys and the trust store (service).
- Keystrokes and clipboard contents in transit.
- The ability to inject input into a machine, including the secure desktop.

## Trust boundaries

| Boundary | Less trusted side | Notes |
|---|---|---|
| LAN → service | Any host on the network | Unauthenticated packets reach `proto` decoding as SYSTEM. |
| Named pipe → service | Agents, UI | The service validates everything it receives (ADR 0002). Agent link: see "Agent → service". |
| Service → winlogon agent | — | Agent runs as SYSTEM on the secure desktop. |

## Threats

### Hostile bytes on the wire (LAN → service)

A host on the LAN, or a paired peer that is buggy or compromised, sends malformed frames or datagrams to make the SYSTEM service crash, hang, or exhaust memory. Connection authentication (`pairing`, `transport`) narrows who can send frames at all; `proto` does not rely on it.

Defenses, specified in `openspec/specs/wire-protocol/spec.md`:

- **Bounded memory.** A frame's payload length is checked against the 65,536-byte limit from the 6-byte header alone, before any payload is buffered. Decoders borrow from the caller's buffer and never allocate from a length the peer claims. Datagrams are capped at 1,200 bytes.
- **Strict parsing.** Unknown message types, types on the wrong channel, and payloads that do not match their layout close the connection with a protocol error, not a best-effort guess. Only `Hello` tolerates trailing bytes, so that later versions can extend it.
- **No panics by construction.** `proto` denies indexing, `unwrap`, `expect`, `panic`, and unchecked arithmetic outside tests; CI builds with `-D warnings`.
- **Tested against arbitrary input.** A deterministic never-panic test runs in every `cargo test`, and cargo-fuzz targets for stream and datagram decoding run nightly in CI.

Residual risk: a connected peer can keep the connection busy with valid traffic. Only members get that far (see "Strangers and spoofing on the LAN"); rate limiting members is left for later.

### Guessing the pairing code (LAN → service)

A host on the LAN connects while a machine is in pairing mode and tries codes. SPAKE2 gives it one online guess per attempt and nothing to brute-force offline; a relay between two pairing machines fails key confirmation, which is bound to the TLS exporter and both certificate keys (ADR 0004, Amendment 2). Pairing mode opens only when the user asks and handles one attempt at a time. It closes after 10 minutes, after one successful pairing, or after 3 failed attempts, and its code works only while it is open. An attempt that stalls for 30 seconds counts as failed, so an attacker cannot hold the single attempt slot for longer. Specified in `openspec/specs/pairing/spec.md`.

Residual risks:

- **Guessing.** At most 3 guesses against a 6-digit secret each time the user opens pairing mode: a 3-in-a-million chance per opening, for a host on the same LAN at that moment.
- **Denial of pairing.** A host that sends 3 wrong guesses closes pairing mode, and one that holds an attempt open delays a legitimate joiner by up to 30 seconds. The user sees "too many wrong codes" and opens pairing mode again; repeated closures point at a hostile host on the LAN.

### Desk membership

Any member of a desk can add a machine: the others accept a membership record signed by any member (ADR 0004, Amendment 2). A compromised member can therefore add a machine of the attacker's choosing. It can already inject input into every member, so this does not widen what a compromised member can do. Removal records are kept, so a removed machine cannot return through a stale membership record.

Defenses against a misbehaving member: a receiver holds records for at most 256 machines, and more than 256 records in one exchange is a protocol error.

Residual risks:

- **Clock disagreement.** A record's epoch is its signer's Unix time. If a removal and a re-pair of the same machine happen closer together than the two signers' clocks disagree, the earlier action can win. A member with a clock far in the future makes its records hard to override until real time catches up; removing that member and pairing again fixes it.
- **Divergence.** A record signed by a member that is removed before the record reaches everyone is accepted by members that saw it first and dropped by the others. Removing and re-pairing the affected machine makes the views agree again.

### Strangers and spoofing on the LAN (LAN → service)

Any host on the LAN can reach UDP 47391 and mDNS. The goals: it learns nothing about the desk, cannot pose as a member, and cannot hold resources for long. Specified in `openspec/specs/peer-transport/spec.md` and `openspec/specs/discovery/spec.md`.

- **Posing as a member is answered by pinning.** A dialer reaching a member accepts only that member's Ed25519 key, and the TLS 1.3 handshake proves the peer holds it. A spoofed mDNS advertisement, or a member's address taken over by another machine, makes the dial fail; nothing is sent.
- **Records are withheld from strangers.** On a member connection, the dialer, which has pinned the listener, sends its records first. The listener sends its own, which carry machine names, only after the dialer has proved to be a member. A stranger gets close code `5` and nothing else.
- **Introduction needs a member's signature.** A key unknown to the listener becomes a member only through a record that an existing member signed (see "Desk membership").
- **Bounded state for strangers.** A connection from a key that is not a member must send `PairPake` or finish introducing itself within 10 seconds, and at most 8 are held at once. More are closed straight after their handshake.
- **What an advertisement reveals:** that a DeskHop machine is at this address; its public key, which is not secret; and whether pairing mode is open right now. It does not reveal the machine's name or the desk's members.
- **Firewall.** `add-installer` must allow inbound UDP 47391 and mDNS (UDP 5353) for the service only.

Residual risks:

- **Handshake cost.** Each stranger handshake costs the service an Ed25519 signature and a verification. The limit on strangers bounds what they hold open, not how fast they handshake; rate limiting is left to the firewall unless it becomes a problem.
- **Liveness tuning.** A host that can drop packets between two members for about a second makes them report each other lost. Losing a peer releases held keys, so this costs availability, not safety.
- **Presence on the LAN.** Advertisements show which hosts run DeskHop, and when someone is pairing.

### Forwarded input (LAN → service)

Engine messages and `Motion` datagrams can move focus, type and click on a machine. Specified in `openspec/specs/input-forwarding/spec.md`.

- **Only members that are up.** Engine messages travel only on member connections, after the peer's `RecordsDone`. One on a pairing connection, or earlier on a member connection, closes it with a protocol error. `Motion` datagrams that arrive before then are dropped. The node also reports a message only from the peer's current up connection, so a dialer being closed with code `5` gets nothing through.
- **Strict layouts.** Every engine message has a fixed length, and undefined side, button and pressed-or-released codes are protocol errors, under the same no-panic rules as the rest of `proto`.
- **No new power for a member.** A member already counts as trusted to drive input. Its input reaches a machine's OS only while that machine's engine has accepted it as the controller, through `FocusEnter` and the crossing rules. The local user takes control back with a key press or deliberate mouse movement (`input-focus`). Running totals can carry arbitrarily large movement, but a member could already send it as many smaller moves.

Residual risk: a compromised member can push focus onto a machine whose layout maps an edge to it, and type there until the local user takes control back. This is the same trust a member has always had; removing the member ends it.

### Agent → service (named pipe)

The session agent runs as the logged-in user and reports every key and mouse event to the SYSTEM service, which forwards physical input to peers. The service treats the agent as less trusted (ADR 0002). Specified in `openspec/specs/ipc-protocol/spec.md` and `openspec/specs/agent-link/spec.md`.

- **Strict decoding, bounded memory.** IPC frames are capped at 65,536 bytes, checked from the header alone. Every payload has a fixed layout, except `Screen`, whose count, identity lengths, rectangles and DPI are all bounded. Decoding is under the same no-panic lints as `proto`, with a never-panic test and a nightly fuzz target (`decode_ipc`).
- **Direction and role are checked.** An agent cannot send a service command, and the service end of an agent link accepts only the session-agent role.
- **A stalled or hostile agent cannot block the service.** The service reads into a buffer holding at most one frame. Sending a command never waits, and an agent that stops reading is cut off after 4,096 queued commands. Events waiting for the first clock exchange are capped at 10,000.
- **The link ending releases input.** The agent returns capture to pass-all and removes its hooks when the link ends. If the agent dies, Windows removes its hooks. Input is never left withheld without a service.

Open item, closed by `add-service`: today nothing checks which process connects to an agent pipe, so a process that reached it could claim physical input, which the service would forward to peers. `add-service` closes this:
- each agent's pipe gets a random name, created before the agent starts, with `FILE_FLAG_FIRST_PIPE_INSTANCE`;
- its ACL allows only SYSTEM and that session's user;
- the service accepts only the process ID it launched (`GetNamedPipeClientProcessId`).

### Identity key at rest

The identity key and trust store are files that only SYSTEM can read. A local administrator can read or replace them. A local administrator can already inject input and read the clipboard, so this is outside the threat model; DPAPI was rejected for the same reason.
