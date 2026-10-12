# ADR 0004 — Symmetric peers, mDNS + pairing code, QUIC over Noise (amended)

**Status:** Accepted (shape); details to be settled in `openspec/` changes. Amendment 1 (pairing) Accepted 2026-09-29. Amendment 2 (QUIC TLS pinned to identity keys; Noise dropped) Accepted 2026-10-01. Layout refined by ADR 0005.
**Date:** 2026-09-27

## Context

Synergy's configuration pain comes from its protocol shape: a designated server, clients pointed at an IP, TLS certificates managed by hand, and a screen grid the user fills in. The zero-config promise requires the protocol to remove each of those, not the UI to hide them.

Input has two traffic classes. Mouse motion is high-rate, loss-tolerant, and stale after a few milliseconds. Keys, focus changes, layout updates, and clipboard are low-rate and must arrive in order.

## Decision

- **Symmetric peers.** No server/client roles. Whichever machine has physical input is the source; the role moves with the cursor.
- **Discovery** by mDNS/DNS-SD, with a **pair-by-code fallback** that carries an IP hint for networks that block multicast.
- **Pairing** with a short code shown on both screens, establishing long-lived identity keys via a Noise handshake (`snow`). The trust store is per machine; no CA, no certificates. *Superseded by Amendments 1 and 2.*
- **Transport** is QUIC (`quinn`): unreliable datagrams for coalesced motion, ordered streams for everything else. One connection per peer pair. *Authentication settled by Amendment 2.*
- **Layout** is a graph of edge→peer transitions, learned by use: the first time the cursor exits an edge, the UI asks which machine is there. Machines with several monitors expose their virtual-screen edges. *Refined by ADR 0005.*
- **`proto`** carries a protocol version in every handshake and is the only crate that defines wire types. It has no I/O.
- **LAN only** in v1. No relay, no NAT traversal.

## Alternatives considered

- **TLS over TCP + raw UDP.** Works on a LAN; rejected because QUIC gives both traffic classes on one authenticated connection with a single handshake.
- **Designated server** (Synergy, Mouse Without Borders). Simpler to reason about; rejected because it is the root of the configuration burden.
- **Clipboard over a separate channel.** Unnecessary; a QUIC stream per transfer suffices.

## Open questions for spec changes

- Noise handshake pattern and pairing-code length/entropy.
- Motion coalescing interval and datagram payload layout.
- Behavior when two machines both have physical input at once.
- How a machine with multiple monitors presents edges during layout learning.
- Fuzzing strategy for `proto` decoding in the service.

## Consequences

- A third party could write a compatible client from `docs/protocol.md`; that is intended.
- Corporate networks that block multicast degrade to pair-by-code, never to typing IP addresses into a grid.
- The engine is unaware of transport; it consumes `model` events and emits routing decisions, which keeps it testable on Linux.

## Amendment 1 — Pairing by typed code with a PAKE

**Status:** Accepted (2026-09-29)
**Date:** 2026-09-28

### Context

The original decision describes a code "shown on both screens", which the user compares. The pair-by-code fallback describes a code the user types on the other machine. These need different cryptography. For v1 one flow is enough, and a typed code covers both the normal and the fallback case.

A short typed code cannot be used directly as a Noise pre-shared key: anyone who records one pairing can brute-force the code offline.

### Decision

- **Pairing uses a typed code.** One machine displays a short code; the user types it on the other. The same flow is used whether or not mDNS found the peer; on networks that block multicast the code also carries an IP hint.
- **The code authenticates through a PAKE** (a password-authenticated key exchange, such as CPace or SPAKE2), so an attacker gets one online guess per pairing attempt and nothing to brute-force offline.
- **Identity keys are unchanged.** Pairing still establishes long-lived identity keys via Noise (`snow`) and stores them in the per-machine trust store. The PAKE authenticates that exchange; how the two compose is settled in the `pairing` spec. *Superseded by Amendment 2: no Noise; identity keys are pinned in QUIC's TLS.*
- The PAKE implementation is a dependency of `pairing` only. `proto`, `model`, and `engine` are unaffected.

### Alternatives considered

- **Compared code** (derived from the handshake hash, shown on both screens, like Bluetooth numeric comparison). Needs no PAKE and resists offline attack. Deferred: v1 uses one flow, and the fallback needs a typed code anyway.
- **Typed code as a Noise pre-shared key.** Simplest; rejected because a recorded handshake allows offline brute force of a short code.

### Consequences

- `pairing` gains one cryptographic dependency, chosen in the `pairing` spec with a stated reason.
- The open question "Noise handshake pattern and pairing-code length/entropy" becomes: which PAKE, how it composes with the Noise handshake, and code length.
- `docs/threat-model.md` records online guessing as the residual pairing risk, with a limit on attempts.

## Amendment 2 — QUIC TLS pinned to identity keys; Noise dropped; desk membership

**Status:** Accepted (2026-10-01)
**Date:** 2026-10-01

### Context

QUIC always runs TLS 1.3; `quinn` uses `rustls`. Adding Noise for authentication would mean two handshakes, or replacing QUIC's TLS with an experimental Noise integration. Amendment 1 left open how the PAKE composes with Noise.

The engine sends input straight to the machine it controls, including after a hand-off across a third machine. Every pair of machines in a desk therefore needs mutual trust, not only the pairs that were introduced.

### Decision

- **Identity.** Each machine generates one Ed25519 key when the service first starts. A peer's identity (`PeerId`) is its 32-byte public key.
- **Sessions.** QUIC with TLS 1.3. Both sides present a self-signed certificate carrying their identity key. A custom verifier accepts the peer only if that key is in the trust store. There is no CA, and certificate fields other than the key are ignored. Noise and `snow` are not used.
- **Pairing.** A machine in pairing mode accepts any certificate, but only for the pairing exchange on that connection. The code authenticates through SPAKE2 in symmetric mode (`spake2` crate); the password is every digit of the code. Each side then sends a key-confirmation MAC, keyed by HKDF over the SPAKE2 key with the TLS exporter output and both certificate keys bound in. A mismatch spends an attempt and closes the connection, so a relay in the middle gains one online guess and nothing more.
- **Code.** All digits: `<locator>-<secret>` plus a check digit. The locator is the host part of the displaying machine's IPv4 address (one byte on a /24, two on a /16). The typing machine adds its own prefix, then finds the peer by mDNS or connects to that address directly. The check digit catches typos before an attempt is spent. Pairing mode is open only while the user has asked for it, for a bounded time, with a bounded number of failed attempts per code. Lengths and limits are set in the `pairing` spec.
- **Desk membership.** A new machine pairs with any one member and joins the whole desk. That member signs a membership record (the new key and machine name) with its identity key. Members accept records signed by any member and pass them on when they connect. Removing a machine, from any member, produces a signed removal record that is kept, so a stale membership record cannot re-add it.
- **Storage.** The identity key and the trust store are files under `%ProgramData%\MouseFlock` that only SYSTEM can read. The installer sets that permission. No DPAPI.
- **Dependencies.** `rustls` (through `quinn`), a self-signed certificate generator, `spake2`, and HKDF belong to `pairing` and `transport` only. `proto`, `model`, and `engine` are unaffected.

### Alternatives considered

- **Noise as QUIC's handshake** (`quinn-noise`). One handshake, but experimental and outside `quinn`'s supported path; rejected.
- **Noise for pairing, TLS for sessions.** Two handshake systems to audit for no gain over pinning keys in TLS; rejected.
- **Pairing each pair of machines.** Simplest trust store, but a desk of n machines needs n(n-1)/2 codes, and an unpaired pair shows up as an unexplained wall after a hand-off; rejected.
- **CPace** (`pake-cpace`). The CFRG-recommended balanced PAKE, but the crate has had no release since 2023. **OPAQUE** (`opaque-ke`) is an augmented PAKE for stored passwords, the wrong shape for a one-time code. Both rejected.
- **DPAPI for the key files.** In machine scope anything running as SYSTEM or admin can decrypt it, and those can already read the files; it also needs `unsafe` Win32 calls. Rejected.
- **Word codes** (`137-maple-river`). Easier to say aloud, but English-only and longer to type; rejected.

### Consequences

- AGENTS.md lists `rustls` and `spake2` in place of `snow`.
- Any member can add a member without the others agreeing. These machines already accept each other's input, so the trust boundary is the desk; `docs/threat-model.md` records it.
- A local administrator can read the identity key. A local administrator can already inject input, so this is outside the threat model.
- IPv6-only LANs pair through mDNS only; the direct-connection fallback needs IPv4.
- The open question "which PAKE, how it composes with the Noise handshake" is closed. Code lengths, time limits, and attempt limits remain for the `pairing` spec.
