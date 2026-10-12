# Proposal

## Why

Text and images copied on one MouseFlock machine already paste on another. Files do not: copying files in Explorer and crossing to another PC leaves them behind. Moving a file between paired PCs is one of the most common things people reach for after text, and the brief promises that "the clipboard follows the cursor". This adds files, by copy and paste, the same gesture people already use.

## What Changes

- **Copied files follow the cursor, pasted on the other PC.** Select files in Explorer on A, Ctrl+C, cross to B, and Ctrl+V in a folder: the files are copied there. This uses the clipboard's existing announce, newest-copy and request model (the `clipboard` capability); this change adds how a file copy is carried.
- **Bytes move when you paste, not when the cursor arrives.** When the cursor crosses into a PC, only a small **manifest** (each file's relative path and size) is fetched, enough to offer the paste. The file **bytes** move only when the user pastes, pulled on demand, file by file. Nothing large moves unless pasted.
  - This is Windows' own delayed-rendering mechanism for virtual files (`CFSTR_FILEDESCRIPTOR` and `CFSTR_FILECONTENTS` served by a live data object), the same way an email client offers an attachment or a zip tool offers a file inside an archive.
  - Explorer shows its own copy progress and lands the files in the folder being pasted into.
- **Multiple files and whole folders.** A selection of several files, and directory trees, are carried; empty folders are recreated. Each file streams on its own, so there is no small size cap as there is for text (4 MiB) and images (32 MiB); a multi-gigabyte file is pulled straight through.
- **Not shared:** a copy with no files; a per-copy total above a high ceiling (default 64 GiB, and at most 65,536 files) to bound a hostile manifest; and files on the secure desktop (the winlogon agent has no clipboard).
- **The service never opens a file.** To the SYSTEM service, a file's bytes are an opaque stream it relays between agents. The session agent, as the user, reads the source files and writes nothing — it only serves bytes; the receiving agent hands a stream to Explorer, which writes the files as the user.
- **Version bumps, staying compatible.**
  - **Peer protocol:** version 3 adds the file manifest, the per-file content request, and the file content stream. This release speaks versions 1–3. A member at version 1 or 2 is never offered files and keeps working.
  - **IPC:** version 3 adds the file messages between service and agent. A service and agent mid-upgrade still work, without files.
- **Docs:** `docs/protocol.md` (new messages, version history), `docs/threat-model.md` (untrusted files, where they are written, path safety), and AGENTS.md.
- **Out of scope:**
  - drag-and-drop across the screen edge (a live cross-machine OLE drag), as agreed with the user: copy and paste only;
  - pasting files into an application other than a file manager (for example attaching to an email) — supported only where Windows renders `CFSTR_FILECONTENTS`, which Explorer does;
  - resuming an interrupted transfer;
  - files on the secure desktop;
  - an off switch or policy.

## Capabilities

### New Capabilities
- `file-transfer`: copying files between paired PCs by paste. It covers reporting a file copy as a manifest, offering and fetching the manifest under the clipboard's model, placing a delayed data object so paste pulls the bytes, fetching each file's contents on demand at paste time, streaming without a small size cap, directory trees, what is not shared, path safety for arriving files, and keeping file bytes out of the log and the service's hands.

### Modified Capabilities
- `agent-link`: the session agent reports a file copy as a manifest and serves a file's bytes on request; the service asks the agent to offer files for paste and streams requested file bytes to it. Image and text runs are unchanged.
- `clipboard`: a connection takes one stream carrying a copy and one carrying a file's contents at the same time, so a long file paste never holds up a text or image copy from the same machine.
- `wire-protocol`: this release speaks peer protocol versions 1 to 3.
- `ipc-protocol`: this release speaks IPC versions 1 to 3.

## Impact

- **Stacking:** this change builds on `add-clipboard-images` (protocol and IPC version 2), which is merged but not yet archived, and on `add-elevated-windows`. The three SHALL be archived in order — images, then elevated windows, then files — so the version-range and spec deltas apply cleanly. The code already runs at version 2.
- **Crates touched:**
  - **`proto`:** version 3; the manifest on a clipboard stream (`ClipboardFilesData`), `FileContentRequest` on the control stream, a file content stream (`FileContentData` + reuse of `ClipboardChunk`), `u64` content lengths, the ceilings.
  - **`transport`:** carry the manifest and the per-file content streams; offer files only on version 3 connections; a request/serve path for content, streamed, at the clipboard stream's low priority.
  - **`ipc`:** version 3; the manifest report and set, and the content request and serve, between agent and service.
  - **`win32-clipboard`:** read `CF_HDROP` on copy and walk the trees into a manifest; place an OLE `IDataObject` offering `CFSTR_FILEDESCRIPTOR` and `CFSTR_FILECONTENTS` as `IStream`s that pull bytes on demand; this is new COM and `unsafe`, in the one clipboard crate where it is allowed.
  - **`bin/agent`:** report file copies, serve source file bytes, and drive the data object's render callbacks over the link.
  - **`bin/service`:** the held copy and the tracker gain a file kind carrying the manifest; relay content requests and streams between the receiving and owning members; log file counts and totals, never names or bytes beyond counts.
  - **`tools/fuzz`:** the new decoders and the manifest parser.
  - **No change:** `engine`, `model`, `pairing`, `policy`, `win32-input`, `win32-session`, `win32-shell`, `apps/desktop`, `installer`.
- **Dependency rules hold:** only `mouseflock-agent` links `win32-clipboard`, so the COM file code stays out of the SYSTEM service; `proto` and `ipc` bump their versions and update `docs/protocol.md`; no new dependency in `proto`, `model` or `engine`.
- **No grid, no server/client toggle, no IP address field.**
- **Testing:**
  - unit and transport tests over loopback, including a version 1/2 peer against a version 3 one, and a directory tree;
  - fuzzing of the manifest and content decoders;
  - a manual check with the user on two PCs: copy a file, a folder, and a large file on one PC and paste on the other.
