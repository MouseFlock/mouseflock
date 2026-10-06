# DeskHop — Design Brief

*Status: settled direction as of 2026-09-27. Decisions are recorded in `docs/adr/`. Behavior is specified in `openspec/`.*

## What it is

A software KVM for Windows. One keyboard and mouse drive several PCs over the LAN; the cursor slides across screen edges, the clipboard follows. Every machine runs the same binary. Whichever machine has physical input is the source at that moment.

## Why it wins

Capable engines already exist (Mouse Without Borders, Deskflow, Input Director). Nobody is winning on the engine. The open slot is **zero configuration that stays reliable at the hard moments**: UAC prompts, the lock screen, RDP sessions, mixed-DPI monitors.

Install on two machines. They find each other. Type the code one shows into the other. Push the cursor off an edge and it crosses; with more than two machines it asks, once, which one is over there. No server/client roles, no IP addresses, no screen grid, no certificates.

## Non-goals

- No Synergy-style configuration surface, ever. Nothing needs arranging before it works; an optional view may show and correct the learned layout (ADR 0005). If a feature needs a setup grid, the design is wrong.
- Windows only. No macOS, no Linux.
- LAN only in v1. No cloud relay.
- No video, no remote desktop.
- No gaming or anti-cheat support. Injected input is flagged as injected; that is fine.
- An LLM has no place in the input path or layout logic. "AI-driven" means a troubleshooting assistant over structured diagnostics, and later natural-language remaps.

## Architecture in one paragraph

A SYSTEM service in session 0 owns the network, pairing keys, and managed configuration, and spawns two agents: a session agent on the user desktop for hooks and injection, and a winlogon agent for UAC and the lock screen. A React UI in a Tauri 2 shell is optional and talks to the service over a named pipe. Peers discover each other over mDNS and speak an encrypted QUIC link. The engine is a pure state machine with no Win32 dependency, testable on Linux.

## Stack

Rust throughout: service, agents, engine, and the Tauri bridge. React + TypeScript for the shell. WiX for the installer.

## Open source

Apache-2.0. The client is complete on its own: no account, no server, no cloud. The DeskHop name and mark are trademarked. For software that injects keystrokes over the network, an auditable client is the point; `unsafe` is confined to four crates, and `proto`, `model`, and `engine` are kept dependency-light as the audit surface.

## MVP

Two PCs, Windows 10 22H2 and 11, x64 and ARM64. Discovery, pairing code, edge transitions with learned layout, keyboard and mouse, text clipboard. Full process topology from day one, so UAC and lock-screen support are a feature flip, not a rewrite.

Build order: `proto` → `model` + `engine` with replay tests → `pairing` + `transport` → `win32-input` in a bare agent → `ipc` → `service` → Tauri onboarding. The cursor crosses between two PCs before any UI exists.

## Open questions for the first spec changes

- Pairing-code lengths, time limits, and attempt limits (ADR 0004, Amendment 2).
- ~~Motion datagram byte layout.~~ Settled: running totals with a sequence number, coalesced to one datagram per 4 ms (`docs/protocol.md`, Motion).
- Which monitor identity survives docks and port changes: EDID or Windows device path (ADR 0005; spike).
- Whether a machine locks or sleeps while its input is being forwarded (spike).
- Managed-configuration schema and its registry source.
