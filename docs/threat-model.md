# DeskHop threat model

*Status: covers the wire, pairing, desk membership, the LAN transport, forwarded input, the agent → service and UI → service pipes, the UI app, the service's data folder, console mode and secure attention. The secure desktop is filled in with its change.*

DeskHop injects keystrokes received over the network, and its service runs as SYSTEM. This document records what it defends against and where.

## Assets

- Identity keys and the trust store (service).
- Keystrokes and clipboard contents in transit.
- The ability to inject input into a machine, including the secure desktop.

## Trust boundaries

| Boundary | Less trusted side | Notes |
|---|---|---|
| LAN → service | Any host on the network | Unauthenticated packets reach `proto` decoding as SYSTEM. |
| Named pipe → service | Agents, UI | The service validates everything it receives (ADR 0002). See "Agent → service" and "UI → service". |
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

- **Only the launched agent is accepted.** Anything that reached an agent pipe could claim physical input, which the service forwards to peers. So, specified in `openspec/specs/session-agents/spec.md`:
  - each launch creates a new pipe whose name carries 128 random bits and is never reused;
  - the pipe is created before the agent starts, as the first and only instance of that name (`FILE_FLAG_FIRST_PIPE_INSTANCE`, one instance), and refuses remote clients;
  - its protected ACL lets only SYSTEM and the session's user open it;
  - the service compares the connecting client's process ID (`GetNamedPipeClientProcessId`) with the process it launched, before reading a byte, and closes a mismatch unread. It keeps the launched process's handle open, so that ID cannot be reused by another process meanwhile.
- **The agent runs as the user, not as SYSTEM.** The service launches it with the session user's own token (`WTSQueryUserToken`, `CreateProcessAsUserW`), so it has the user's rights and no more. Every launch goes through one function in `bin/service`, and the Win32 calls live in `win32-session` (ADR 0006).

Residual risk: another process running as the same user can open the pipe if it wins the race to connect. It is then closed unread, and that launch counts as failed. Such a process could keep the agent from linking, a denial of service, but it cannot inject or report input. A process running as the user can already synthesize input on that user's desktop, so the pipe adds no power.

### UI → service (named pipe)

A UI can pair this machine with another, remove members, and change the learned layout. Pairing adds a machine that can then type into this one, so who can reach the UI pipe matters. Specified in `openspec/specs/ui-link/spec.md`.

- **Who can connect.**
  - The pipe `\\.\pipe\deskhop-ui` refuses remote clients.
  - Its protected ACL admits SYSTEM and interactive users (`IU`).
  - The service then accepts a client only if the connecting process runs in the active console session, before reading a byte from it. A UI in an RDP session, or in a session switched away from the console, is closed.
  - In console mode the pipe admits only the user running console mode.
- **What a UI can do.** Any user at the console may open pairing, join, remove a member, undo a learned side and forget the layout. This was chosen for zero configuration: pairing still needs physical access to both screens and a one-time code with 3 guesses, so a user at this console can already decide what this machine is joined to. Managed configuration can narrow it later.
- **What a UI sees.** The pairing code while pairing is open, member names and keys, and where focus is. Everyone at the console can see the screen anyway.
- **A slow or hostile UI cannot block the service.**
  - At most 8 UIs connect at once, enforced by the pipe's instance limit.
  - Every message has a fixed or bounded layout (names up to 63 bytes, codes up to 32, at most 256 members), under the same no-panic decoding and fuzzing as the rest of `ipc`. A UI that breaks it is closed alone.
  - Sending to a UI never waits, and a UI with more than 256 unsent messages is disconnected.
  - Commands are cheap, and pairing attempts are limited by pairing mode itself.
- **Pipe squatting.** A process that created `\\.\pipe\deskhop-ui` before the service could pose as the service to UIs: show a wrong code, or collect a typed one.
  - The service starts at boot and creates the name as the first instance before anyone logs on. It keeps one instance listening at all times, so the name never falls free while it runs.
  - If the name is taken at start, the service logs it and retries every 5 seconds.
  - **The UI checks the server before sending anything.** Before its `Hello`, `DeskHop.exe` asks for the session of the process serving the pipe (`GetNamedPipeServerSessionId`, in `win32-shell`, ADR 0007). It goes on only if that is session 0, where only services run; anything else is dropped unread, and the UI shows that the service isn't running. A squatter running as a user cannot run in session 0, and a process in session 0 is a service an administrator installed, who is outside this threat model. Specified in `openspec/specs/desktop-app/spec.md`.
  - **Development builds** also accept a server in their own session, so they work with `deskhop-service --console`. Release builds, the only ones the installer ships, never do.

**The UI app itself** (`DeskHop.exe`) runs as the user and holds no keys. Pairing codes pass through it only as they are shown or typed.
- **The webview** loads only the app's own pages (CSP `default-src 'self'`), never remote content. It can call only DeskHop's own commands; no file, shell or HTTP plugin is included. The Undo popup's window may call only `view` and `undo`.
- **Start with Windows** is a value under the user's own `Run` key. Anything running as the user could change it, but such a process could already run anything as that user.

Residual risks:

- **Pairing by anyone at the console.** On a shared or family PC, any user who can log on can pair a machine of their choosing. Removing it again is just as easy for the owner.

### Identity key at rest

The identity key and trust store are files that only SYSTEM can read. A local administrator can read or replace them. A local administrator can already inject input and read the clipboard, so this is outside the threat model; DPAPI was rejected for the same reason.

Defenses for the data folder (`%ProgramData%\DeskHop`), specified in `openspec/specs/service-host/spec.md`:

- **Owned by SYSTEM before anything is read.** `ProgramData` lets standard users create folders, so a user could create `DeskHop` first and plant an identity, a trust store or layouts in it. At every start, before reading a file, the service checks the folder:
  - owned by SYSTEM or Administrators: its owner is set to SYSTEM and its protected, SYSTEM-only ACL is applied again;
  - anything else (another owner, a junction or symbolic link, a file): it is moved aside unread and a new folder is created.
- **Atomic, checked files.** Every file is written to a temporary file and renamed over the old one. A file that does not parse, or is larger than 1 MiB, is moved aside and treated as absent; nothing partial is used. A damaged identity costs the desk (peers must pair again), which is logged as an error.
- **The log is readable by administrators.** The log file carries its own ACL (SYSTEM full control, Administrators read), so an elevated administrator can read it by path, while the folder and the other files stay SYSTEM-only. Logs never hold pairing codes, keys or input contents.

### Console mode (development)

`deskhop-service --console` runs as the logged-in user and keeps its data in the user's profile (`%LOCALAPPDATA%\DeskHop\console`), never in the service's folder. Its identity and trust store are therefore as safe as the user's own files: any process running as that user can read them, and could pose as this console-mode machine to its peers. It is a development tool, and the registered service never reads its files.

### Secure attention (Ctrl-Alt-Del)

A controlling peer can ask a machine to show its secure attention screen. The registered service calls `SendSAS` only if the `SoftwareSASGeneration` policy allows services (value 1 or 3), and logs it otherwise. The development script sets the policy to 1 when it is unset, as the installer will. `SendSAS` shows the Ctrl-Alt-Del screen and injects nothing. A member can already control the machine, so the request adds no power. Console mode cannot call it and logs that.
