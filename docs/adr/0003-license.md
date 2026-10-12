# ADR 0003 — Apache-2.0

**Status:** Accepted
**Date:** 2026-09-27

## Context

The project is open source. For software that injects keystrokes over the network, an auditable client is a core property, not a nicety: users and IT departments need to be able to read the code that runs as SYSTEM on their machines.

The license must be decided before the first external contribution. Changing it later means obtaining consent from every contributor or removing their code.

## Decision

1. The `mouseflock` repository (engine, protocol, service, agents, Tauri client, installer) is licensed **Apache-2.0**.
2. The **MouseFlock** name and mark will be registered as trademarks. Forks are welcome under the license but may not use the name.
3. No contributor license agreement. Contributions are accepted under the repository license via the standard Apache-2.0 inbound=outbound terms.
4. The client is complete on its own: no account, no server, no telemetry, no network dependency beyond the peers on the LAN.

## Alternatives considered

- **GPL-3.0.** Stronger copyleft; adds friction for anyone embedding the engine or protocol crates and for contributors from some organizations. Rejected.
- **MIT.** Equivalent in practice; Apache-2.0 adds an explicit patent grant, which matters for a project other companies may build on. Rejected in favor of Apache-2.0.
- **Source-available licenses (BSL, FSL).** Not open source; would undercut the audit argument and deter contributors. Rejected.

## Consequences

- Anyone may fork, redistribute, and build on the code. The project's identity rests on the trademark and on being the reference implementation.
- `docs/protocol.md` is written so that a third party could implement a compatible peer; that is intended.
- Signing keys and release infrastructure are outside the repository and are not a licensing concern.
