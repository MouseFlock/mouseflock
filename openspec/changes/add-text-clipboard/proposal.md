# Proposal

## Why

The design brief promises that "the clipboard follows", and text clipboard is part of the MVP. Today a user can drive two PCs with one keyboard and mouse but cannot copy on one and paste on the other. `win32-clipboard` is an empty crate, and neither the wire protocol nor the agent link carries clipboard data.

## What Changes

- **Copied text follows the cursor**, as agreed with the user.
  - A copy on one PC sends only a small announcement to the other members, never the text.
  - When the cursor crosses into a PC, that PC fetches the newest copy if another member holds it, and puts it on its own clipboard.
  - Text therefore reaches only the PCs the user actually goes to.
- **The newest copy wins.** Each PC treats the most recent copy it has heard of, its own or a member's, as current.
- **Text only, up to 4 MiB of UTF-8.** Images, files and rich text are not shared; for those, the PC keeps what it had.
- **Private copies stay private.** Text that password managers and similar apps mark as excluded from clipboard monitoring, history or cloud sync is never announced or sent. A newer private copy also stops an earlier copy from the same PC from being served.
- **No off switch in this change**, as agreed with the user. Managed configuration can add one later.
- **Transfers never delay input.** Each transfer runs on its own QUIC unidirectional stream, as ADR 0004 says ("a QUIC stream per transfer"). Keystrokes on the control stream never wait behind a large copy.
- **Wire protocol (version 1, not shipped, so no bump):**
  - on the control stream: `ClipboardOffer` and `ClipboardRequest`, carrying a copy's ID;
  - on a clipboard stream: `ClipboardData` (ID and length), then `ClipboardChunk` frames.
- **Agent link (IPC version 1, not shipped, so no bump):**
  - agent → service: `ClipboardText` (length), then `ClipboardChunk` frames, or `ClipboardNotShared` with a reason (private, not text, too large);
  - service → agent: `SetClipboard` (length), then `SetClipboardChunk` frames.
- **The session agent watches and sets the clipboard** through `win32-clipboard`:
  - a listener thread with its own hidden window (`AddClipboardFormatListener`);
  - Unicode text in and out;
  - checks for the privacy markers;
  - its own changes are never reported back, so text never echoes.
- **Docs:** `docs/protocol.md` gets the new types and the clipboard stream, and `docs/threat-model.md` covers clipboard data on the LAN, on the agent pipe and in the service's memory.

## Capabilities

### New Capabilities
- `clipboard`: how copied text moves between members. It covers what is announced and when it is fetched, newest-copy-wins, what is never shared, the transfer stream and its limits, and keeping text out of logs and off disk.

### Modified Capabilities
- `agent-link`: the agent reports copies (text, or why one is not shared) and sets the clipboard on the service's command, without reporting its own change back. These are added requirements; the existing ones are unchanged.

## Impact

- **Crates touched:**
  - **`win32-clipboard`** (`unsafe` allowed, linked only by `bin/agent`): the listener thread, reading and writing Unicode text, the privacy markers, and ignoring its own writes.
  - **`proto`:** four message types, and a clipboard-stream channel in the registry. No new dependencies.
  - **`transport`:**
    - accepts and opens unidirectional clipboard streams on member connections, with one incoming at a time;
    - reports offers, requests and received text to the service;
    - adds `NodeHandle` calls to offer, request and send text.
  - **`ipc`:** the five agent-link messages, with chunked runs bounded at 4 MiB.
  - **`bin/service`:** a pure clipboard tracker (newest copy, when to fetch, what to serve), unit-tested, plus host wiring and tests with two hosts over loopback.
  - **`bin/agent`:** runs the clipboard thread, reports copies, and applies `SetClipboard`.
  - **`tools/fuzz`:** `decode_ipc` covers the new IPC messages and runs, and `decode_stream` covers the new peer messages and the clipboard stream.
  - **Docs:** `docs/protocol.md` and `docs/threat-model.md`.
  - **No change:** `engine`, `model`, `policy`, `pairing`, `win32-input`, `win32-session`, `win32-shell`, `apps/desktop`.
- **Dependency rules hold:**
  - `engine` stays free of `win32-*`, `transport` and `ipc`, and is not touched at all. The clipboard follows focus changes the host already sees.
  - Only `deskhop-agent` links `win32-clipboard`.
  - `proto`, `model` and `engine` gain no dependencies.
- **No grid, no server/client toggle, no IP address field.**
- **Out of scope:**
  - images, files and rich text;
  - pasting on a PC through its own keyboard without the cursor crossing to it first;
  - clipboard on the secure desktop (the winlogon agent);
  - an off switch or policy.
