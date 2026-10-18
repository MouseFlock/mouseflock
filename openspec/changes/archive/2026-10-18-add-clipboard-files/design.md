# Design

## Context

- **The clipboard path today** (add-text-clipboard, add-clipboard-images):
  - A copy is announced with `ClipboardOffer` (ID only), the newest wins, and a machine fetches it when the cursor arrives by sending `ClipboardRequest(id)`; the owner replies on a unidirectional clipboard stream.
  - A stream carries one transfer: a header (`ClipboardData` for text, `ClipboardImageData` for an image) with a `u32` length, then `ClipboardChunk` frames. `transport`'s `TransferReader` checks order and the `u32` limit.
  - The service's `Tracker` holds the newest copy as `transport::Contents` (`Text(Arc<str>)` or `Image(Arc<[u8]>)`), offers it, and fetches on arrival. The node offers a kind only to connections that agreed a version defining it.
  - `win32-clipboard` sets a copy with static `SetClipboardData` (text and a bitmap/PNG). It does **not** use OLE; there is no delayed rendering.
  - Protocol and IPC are at version 2 (images). `Hello` negotiates the highest common version.
- **Files are different in three ways** that shape this design:
  1. They can be huge and many, so a copy cannot be fetched whole on arrival the way text and images are. The bytes must stream, per file, and only when wanted.
  2. "Only when wanted" is **paste**, which is Windows' delayed rendering: the receiver offers a data object, and the file manager pulls each file's bytes through it on paste.
  3. The data object pulls bytes **synchronously from the receiver's side**, which must reach back across the network to the owner — a request/serve path the clipboard does not have today.
- **Settled with the user:** copy and paste (not drag-and-drop); bytes move at paste; files land in the folder pasted into.

## Goals / Non-Goals

**Goals:**
- Every requirement in `specs/file-transfer` and the deltas to `agent-link`, `wire-protocol` and `ipc-protocol`.
- Reuse the clipboard's offer, newest-copy and request model and the stream/priority plumbing; add only what files need.
- Keep file bytes streaming, never whole in memory, and out of the SYSTEM service's hands.

**Non-Goals:**
- Drag-and-drop across the edge; pasting into non-file-manager apps; resume; the secure desktop; a policy.
- Changing text or image behavior.

## Decisions

### D1. A file copy is a third `Contents` kind, carrying its manifest

`transport::Contents` gains `Files(Arc<FileManifest>)`. The manifest is the list of entries — relative path, directory flag, size — and is small (metadata, not bytes). Everything the clipboard already does stays kind-blind: the offer carries only an ID, the newest copy wins across kinds, and the node offers a file copy only on connections at version 3 or later (`ClipKind::Files.since() == 3`).

What a `ClipboardRequest(id)` returns differs by kind: for text and images the owner streams the contents; **for a file copy the owner streams the manifest**, not any file's bytes. So arrival fetches the manifest and nothing more.

### D2. The manifest on the wire

- **Peer:** the owner answers a `ClipboardRequest` for a file copy with a clipboard stream, as for text and images, whose header is a new `ClipboardFilesData { id, len: u32 }` (`0x0016`, version 3) and whose `ClipboardChunk` frames carry the encoded manifest. A manifest can be far larger than a control-stream frame (64 KiB), and on the control stream it would hold up input; on a clipboard stream it cannot.
- **Encoding:** `count: u32`, then per entry `is_dir: u8` (0 or 1), `size: u64` (0 for a directory), `path_len: u16`, and the path, UTF-8, relative to the copy root, with `/` separators. `len` is at most 4 + 65,536 × 788 = 51,642,372 bytes, the most the entry-count and path-length limits allow (D6), so no further ceiling is needed.
- **Parsing:** `proto` parses a manifest in place, without allocating, and checks it whole (count, ceilings, every path, directory sizes, no trailing bytes) before any entry is used.
- The receiving service hands the manifest to its agent to offer (D3), keeping a copy in the `Tracker` so it can map a paste's file index to the owning member and ID.

### D3. Delayed rendering in `win32-clipboard` (the hard part)

On arrival the receiving agent places an OLE data object, not static data:
- The data object lives on a **thread of its own**, an STA that calls `OleInitialize` and pumps messages. Explorer's calls, including each `IStream::Read`, arrive on the thread that placed the object, and a `Read` blocks it while bytes are on their way (found in the task 1 spike). On its own thread that wait never holds up the clipboard thread, so copies made during a long paste are still reported.
- The agent builds an `IDataObject` offering:
  - `CFSTR_FILEDESCRIPTORW` — a `FILEGROUPDESCRIPTORW` from the manifest: one `FILEDESCRIPTOR` per entry, with the relative name, the directory attribute, and the size. Windows uses this to create the tree and show progress.
  - `CFSTR_FILECONTENTS` — per index, as `TYMED_ISTREAM`. Each file's `IStream` is a MouseFlock object whose `Read` pulls the next bytes of that file from the service, blocking until they arrive, and reports end at the file's length.
- `OleSetClipboard(data_object)`. When the user pastes, Explorer reads the descriptor, then for each file opens its `CFSTR_FILECONTENTS` `IStream` and reads it to a new file under the paste target. Explorer shows its own copy dialog and handles name collisions.

The `IStream` is where a paste reaches back to the owner (D4). It is a small, self-contained COM object; the `unsafe` and COM live only in `win32-clipboard`, the one clipboard crate that allows them.

Alternative considered: `TYMED_HGLOBAL` (whole file in one buffer). Rejected — it would hold each file whole in memory and give no progress. `IStream` streams.

### D4. Pulling a file's bytes on paste

When a file's `IStream::Read` needs more bytes:
1. The agent, if it has not started this file, sends the service `FileContentRequest { index }` for the currently set file copy, over the agent link.
2. The service maps the set copy to its owner and ID and sends the owner `FileContentRequest { id, index: u32 }` (`0x0017`, version 3) on the control stream; it is 12 bytes, so it does not hold up input.
3. The owner's service asks the owner's agent to serve that file; the owner's agent opens the source file and streams it.
4. The bytes flow back: owner agent → owner service → QUIC **file content stream** → receiver service → receiver agent → the waiting `IStream::Read`.

**Two lanes.** A connection takes two incoming unidirectional streams at once (QUIC's stream limit goes from 1 to 2): at most one carrying a copy (text, image or manifest) and at most one carrying a file's contents. A second of either kind that arrives while one of its kind is open is left unread until the first is done (QUIC flow control holds its bytes at the sender), so the memory bound for copies is unchanged. It is not an error: quinn reports a stream done when the peer acknowledged its data, not when the peer finished reading it, so an honest sender can overlap by a moment; and a file's contents are streamed, never held. The sender keeps one stream of each kind at a time per connection, waiting its turn. So a long paste never holds up a text copy from the same machine (chosen with the user; it modifies the `clipboard` capability's stream rule).

The file content stream is a unidirectional stream like a clipboard stream, at the same low priority, starting with `FileContentData { id, index: u32, len: u64 }` (`0x0018`, version 3) then `ClipboardChunk` frames. Its length is `u64`, so a file over 4 GiB is carried. `proto`'s `TransferReader` takes this as a third kind of header, counting a `u64` remainder, with no small cap; text, image and manifest headers keep their `u32` limits. One stream carries one file.

The agent serves at most one requested file at a time per copy; Explorer reads files in order. If the owner goes down or the file cannot be read, the stream is reset, the `IStream::Read` returns an error, and Explorer reports the paste failed (D handles the spec's "owner lost mid-paste").

### D5. IPC (version 3)

The file messages take their own range, `0x0040`–`0x004F`. Each direction has one message ending a run, from the side sending the bytes, and one asking for that, from the side receiving them, so an agent that serves one file while receiving another is never unsure which is meant:

| Type | Name | Direction | Payload |
|---|---|---|---|
| `0x0040` | `ClipboardFiles` | agent → service | `len: u32`: a manifest run follows in `ClipboardChunk`s |
| `0x0041` | `FileWanted` | agent → service | `set: u32, index: u32`: the receiving agent wants this file of the set copy |
| `0x0042` | `FileContent` | agent → service | `index: u32, len: u64`: the owning agent starts serving this file |
| `0x0043` | `FileChunk` | agent → service | 1–65,536 bytes of the file being served |
| `0x0044` | `FileAbandoned` | agent → service | `index: u32`: the owning agent stops serving this file, ending its run |
| `0x0045` | `FileConsumed` | agent → service | `n: u32`: the receiving agent passed on `n` more bytes |
| `0x0046` | `FileUnwanted` | agent → service | `index: u32`: the receiving agent no longer wants this file |
| `0x0048` | `SetClipboardFiles` | service → agent | `len: u32`: a manifest run follows in `SetClipboardChunk`s |
| `0x0049` | `SendFile` | service → agent | `copy: u32, index: u32`: serve this file of the copy you reported |
| `0x004A` | `SetFileContent` | service → agent | `index: u32, len: u64`: the wanted file's bytes start |
| `0x004B` | `SetFileChunk` | service → agent | 1–65,536 bytes of the file being set |
| `0x004C` | `AbandonFile` | service → agent | `index: u32`: the file being set will not complete, ending its run |
| `0x004D` | `FileAcked` | service → agent | `n: u32`: the service passed on `n` more bytes |
| `0x004E` | `StopFile` | service → agent | `index: u32`: stop serving this file |

- **The manifest** is a run like text and images, with its own length bound (the peer protocol's, 51,642,372 bytes); `ipc` checks only the length, and the service and agent check the manifest itself with `proto`'s rules.
- **Copy numbers.** Each end numbers the file copies reported on a link (`copy`) and the file copies set on it (`set`), from 1, in order. `SendFile` and `FileWanted` name the number, so an agent never serves a file of a newer copy, or renders one of an older, when a request crosses a new copy on the pipe.
- **File runs** are separate from the clipboard run, with their own chunk types, so a copy can be reported or set while a file streams. One file run at a time in each direction; it ends when its `u64` length is reached, or when the side sending its bytes abandons it.
- **Flow control.** The pipe's queues never wait, so a file run is windowed: whoever sends a file's bytes keeps at most 1 MiB that the other end has not acknowledged with `FileConsumed`; more is a protocol error. The service acknowledges an owning agent's bytes as the network takes them; the receiving agent acknowledges as Explorer reads them. Memory on each side stays bounded whatever the speed of the disk, the network or the paste.

Manifests and file messages never travel on a winlogon link.

### D6. Limits and path safety

- **Ceilings:** at most 65,536 manifest entries; at most 68,719,476,736 bytes (64 GiB) total. Checked by the receiver before building the data object, to bound a hostile manifest. No per-file cap.
- **Path length:** an entry's relative path is at most 259 UTF-16 code units, because a `FILEDESCRIPTORW` name holds 260 with its NUL (found in the task 1 spike). The copying agent reports a copy with a longer path as not shared, whole, and the receiver rejects a manifest with one, so a paste is never silently incomplete.
- **Path safety:** every manifest path is checked before use — it must be relative, use no drive, and never contain a `..` component or an absolute root. A manifest with a bad path is rejected whole and nothing is written. Windows writes the files via the descriptor's relative names under the paste target, so a sanitized manifest cannot escape it. A pure function `sanitize_manifest` does the check and is unit-tested with traversal, absolute and drive cases.

### D7. The service never opens a file

The service relays bytes. The owner's **agent** (as the user) opens and reads source files; the receiver's **agent** (as the user) hands bytes to Explorer, which writes them. The service holds at most the small manifest and one in-flight file's buffered chunk per connection; it never touches the filesystem for a transfer.

### D8. Threat model

`docs/threat-model.md` gains a "File transfer" part under the clipboard:
- **Who reads and writes files:** only the agents, as the user, never the SYSTEM service.
- **Untrusted manifest:** bounded entry count and total, and path-safety checks, so a member cannot make the receiver write outside the paste target or exhaust it before the user even pastes.
- **Untrusted bytes:** a received file is whatever the member sent; the user pasted it, as they would a download. Not executed, just written where they chose.
- **What a member gains:** a member controlling a machine can already type and read its screen; pasting a file they offered is the user's action, in the user's chosen folder.
- **Service memory:** bounded as in D7.

### D9. Checking the result

- **Unit tests:** `proto` manifest and file-content decoders (u64, limits, order); `sanitize_manifest`; `ipc` version-3 messages and run rules; the data object's descriptor built from a manifest (pure part).
- **Transport/host tests over loopback:** a manifest round-trips at version 3; a version-2 node is never offered files and stays up; a file content stream streams; a tree's manifest and per-index fetch; input is not delayed during a transfer.
- **Fuzz:** the manifest parser and the file-content reader.
- **The user, on two PCs:** a 0.3.0-plus build installed as an upgrade (also covers add-installer 6.3's "upgrade keeps pairing"); copy a single file, a folder with subfolders, and a multi-gigabyte file on one PC and paste on the other; confirm nothing moves until paste, and the log holds no names.

## Risks / Trade-offs

- **[Delayed-render COM across a network is intricate]** → The `IStream` blocks a paste on a network round trip. Explorer expects slow streams (it uses them for network shares) and shows progress. The transfer runs at low priority so input is unaffected. Kept to one in-flight file.
- **[A paste that blocks forever if the owner stalls]** → The content stream has the same idle timeout as a clipboard fetch; on timeout the `IStream` errors and Explorer reports failure.
- **[A hostile manifest]** → Bounded (D6) and path-checked before anything is written.
- **[Pasting into a non-Explorer target]** → Only targets that read `CFSTR_FILECONTENTS` work; others see nothing. Documented as out of scope.
- **[A second clipboard thread]** → The files thread only places and serves the data object; the existing clipboard thread keeps listening, reading and setting text and images. Placing files is a clipboard change like any other, which the listener recognises as the agent's own.

## Migration Plan

- **Rollout:** an upgrade installs the new agent and service; files work once both PCs are on this version. A PC on an older version keeps working, without files.
- **Rollback:** reinstall the older version; version negotiation handles the mix.
- **Archive order:** this change stacks on `add-clipboard-images` and `add-elevated-windows`; archive those first so the version-range and spec deltas apply cleanly.

## Open Questions

- Whether some file managers other than Explorer that read `CFSTR_FILECONTENTS` need `TYMED_HGLOBAL` as well as `TYMED_ISTREAM`. The task 1 spike showed Explorer needs only `TYMED_ISTREAM`; other file managers are out of scope.
