# Tasks

## 1. Spike: delayed-render file paste in `win32-clipboard` (de-risks the rest)

- [x] 1.1 In `win32-clipboard`, behind an ignored desktop test, place an OLE `IDataObject` offering `CFSTR_FILEDESCRIPTORW` for one file and `CFSTR_FILECONTENTS` as a `TYMED_ISTREAM` whose `Read` returns fixed bytes (D3). The clipboard thread does `OleInitialize` and keeps pumping. Verify by hand with the user on this PC: paste in Explorer writes a file with those bytes, and the paste shows progress. Record whether `TYMED_HGLOBAL` is also needed (Open Question). If delayed rendering cannot be made to work, stop and report; the fallback would be cursor-arrival copy into a folder, a spec change.

  Done 2026-10-09 in `win32-clipboard/src/files.rs` (`descriptor`, `data_object`, a forward-only `IStream`), with three desktop tests:
  - read back through `OleGetClipboard` from another STA thread: the descriptor matches, no file is opened before its stream is asked for, and a 3 MiB stream reads back byte for byte;
  - dropped on the target folder's own drop target (`BHID_SFUIObject`), as Explorer's Paste does: the tree, the empty folder and a 64 MiB file arrive byte for byte;
  - by hand with the user, Ctrl+V in Explorer: the tree and a 256 MiB file throttled to 32 MiB/s arrived byte for byte in 8 seconds, pulled as produced, and Explorer showed its progress dialog.

  Findings:
  - `TYMED_ISTREAM` alone is enough for Explorer; `TYMED_HGLOBAL` is not needed (resolves the Open Question for Explorer).
  - The shell creates intermediate folders on its own; the manifest still lists every directory, so empty ones are recreated.
  - `IFileOperation::CopyItems` does not take virtual files (it asks for a shell ID list); only paste and drop do. Not needed.
  - A descriptor name holds at most 259 UTF-16 units, so a relative path longer than that cannot be offered.
  - Explorer's `Read` calls arrive on the thread that placed the object and block it while the source waits; the real data object needs a thread of its own (5.2).

## 2. Peer protocol version 3 (`proto`)

- [x] 2.1 Bump `PROTOCOL_VERSION` to 3 (minimum stays 1). Add `ClipboardFilesData` (`0x0016`, clipboard stream), `FileContentRequest` (`0x0017`, control stream) and `FileContentData` (`0x0018`, clipboard stream), all version 3, to the registry (D2, D4), with `MAX_MANIFEST_ENTRIES` (65,536) and `MAX_FILES_TOTAL` (64 GiB). Verify unit tests: version 1 and 2 registries unchanged; version 3 adds exactly these; `Hello` encodes `01 00 03 00`; negotiation with a 1–2 peer agrees 2.
- [x] 2.2 Add the manifest codec (entry: `is_dir`, `size: u64`, `path_len: u16`, UTF-8 path) with the entry-count and total ceilings and the path checks (reject absolute, drive, or `..` paths) as pure functions, parsing in place. Extend `TransferReader` for a manifest header (`ClipboardFilesData`, `u32` length within the manifest bound) and a file content header (`FileContentData { id, index, len: u64 }`), then chunks totalling `len`, no small cap. Verify with unit tests: a manifest round-trips; over-count and over-total are rejected; traversal/absolute/drive paths and paths over 259 UTF-16 units are rejected; a `u64` content stream of more than 4 GiB of header length is accepted; order and overrun are protocol errors.
- [x] 2.3 Extend `tools/fuzz` (`decode_stream` for the manifest and content reader at versions 1–3). Verify it runs 60 seconds without findings.

## 3. Transport (`transport`)

- [x] 3.1 Add `Contents::Files(Arc<FileManifest>)`; a `ClipboardRequest` for a file copy replies with the manifest, not contents (D1). Offer a file copy only on version 3 connections. Verify loopback tests: a manifest round-trips between two version 3 nodes; a version 2 node is never offered a file copy and stays up.
- [x] 3.2 Add the per-file content path (D4): `FileContentRequest` on the control stream, and a file content stream carrying one file. Two lanes: allow two incoming uni streams, at most one copy stream and one file content stream read at a time (a second of a kind waits unread), and send at most one of each kind at a time per connection. Reads feed the node; the owner serves from a request. Verify loopback tests: a requested file streams end to end; a 5 GiB-length stream is carried without buffering whole; input on the control stream is delivered within 50 ms during a transfer; a reset mid-file surfaces as an error; a text copy crosses while a file streams; a second concurrent copy stream is read after the first.

  Done: `Contents::Files` carries the encoded manifest (checked whole on receipt; a bad one is a protocol error). `NodeHandle::request_file` returns a `FileFetch` (bounded, `Len`, `Bytes`, `End`); the owner gets `Event::FileRequest` and answers with `send_file`, a bounded `FileSink`; a short or overlong file resets the stream. Each stream's header is read exactly, so a stream waiting for its lane keeps its stream credit spent. Loopback tests in `transport/tests/files.rs`; the "5 GiB" case announces a 5 GiB length and shows the owner stalls within a bounded amount when nobody reads, then resumes, rather than sending 5 GiB. An owner that abandons a file before its header arrives leaves the fetch to the caller's timeout (documented on `FileFetch`).

## 4. IPC version 3 (`ipc`)

- [x] 4.1 Bump `IPC_VERSION` to 3. Add the file messages of D5 (`0x0040`–`0x004E`), all version 3, never on a winlogon link: the manifest runs, copy numbers, `u64` file runs with their own chunks, and the 1 MiB window. Reasons extend to "no text, image or file". Verify link tests: manifest and content round-trip; a copy and input interleave with a file run; past the window closes the link; a file message on a version 1 or 2 link closes it; a manifest on a winlogon link closes it; the never-panic test covers the new types.
- [x] 4.2 Update `docs/protocol.md`: the new peer and IPC message rows and codes (version 3), the file-run rules, the version-history rows for peer 3 and IPC 3. Verify every row matches the registries.
- [x] 4.3 Extend `tools/fuzz` (`decode_ipc`) for the new types. Verify it runs 60 seconds without findings.

## 5. Reading and offering files (`win32-clipboard`, `bin/agent`)

- [x] 5.1 Read `CF_HDROP` on copy and walk each path into a manifest (relative paths, directories, sizes), with the ceilings; report it only when there is no shareable text or image (D1, D2). Verify unit tests of the walk (a tree with an empty subfolder; the ceilings; a path over 259 UTF-16 units makes the copy not shared) and an ignored desktop test: copying files is reported as a manifest, not text or an image.
- [x] 5.2 Build the real delayed data object from a received manifest (from the 1.x spike), on a files thread of its own (D3), serving each file's `CFSTR_FILECONTENTS` `IStream` from bytes the link supplies, and serving source-file bytes on request (D3, D4). Keep the descriptor build and path handling as pure, tested functions. Verify: unit tests of the `FILEGROUPDESCRIPTORW` built from a manifest; ignored desktop tests that an offered file pastes, and that a source file is served; clippy clean and every `unsafe` block has a `// SAFETY:` comment.
- [x] 5.3 In `deskhop-agent`, report file copies on version 3 links (and as not-shared on older links), serve requested source files, place the data object on a set manifest, and request file contents on paste. A corrupt or rejected manifest leaves the clipboard unchanged and the link up. Verify agent link tests over a pipe pair: a file copy reaches the service end as a manifest at version 3 and as not-shared at version 2; a served file round-trips; a bad manifest keeps the link up.

## 6. The service (`bin/service`)

- [x] 6.1 The `Tracker`'s held and received copies carry the file kind (the manifest and, for a received copy, the owner and ID to map a paste's index). Offer files only to version 3 members. Log file count and total only (D7). Verify unit tests: a file offer skips a version 2 member; the newest copy wins across text, image and file kinds; a paste index maps to the right owner.
- [x] 6.2 Wire the manifest and content request/serve between the agent link, the `Tracker` and the transport in `host.rs`, relaying bytes unopened (D7). Verify host tests over loopback: copy files on A, cross to B, B is set a manifest; pasting index i fetches that file from A; a version-2 member receives no file offer and stays up; neither log holds a file name.

## 7. Docs

- [x] 7.1 Add the "File transfer" part to `docs/threat-model.md`'s clipboard section (D8), update its status line, and add the file gesture to AGENTS.md if the clipboard is described there. Verify by reading against `specs/file-transfer` and D6–D8.
- [ ] 7.2 Update `docs/known-issues.md` only if the 8.2 check finds a limitation users will hit (for example pasting into a non-file-manager app). Otherwise record "no change".

## 8. Integration

- [ ] 8.1 Run the full checks and verify each passes: `cargo fmt --all --check`, `cargo archcheck`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; the Linux set; the ARM64 check; `npm test`; the CI run including the `installer` job.

  Local checks pass (2026-10-09): `cargo fmt --all --check`; `cargo archcheck` (15 packages ok); `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`, every suite passing, and the ignored desktop tests of `win32-clipboard` (16, Explorer-by-hand excluded); the Linux set run here (353 tests); the ARM64 check; `npm test` (21). Fuzzing: `decode_stream` and `decode_ipc` 60 s each, clean. Open until the CI run, including the `installer` job, passes on the pushed branch.
- [ ] 8.2 Manual check with the user on both PCs, on a build installed as an upgrade (also covers add-installer 6.3's upgrade-keeps-pairing):
  - copy a single file on A, cross to B, paste into a folder — it appears, and the service log shows a file offer with a count, no name;
  - nothing moved until the paste (watch that a large file does not transfer on cursor arrival);
  - copy a folder with subfolders (including an empty one) and paste — the tree is recreated;
  - copy a multi-gigabyte file and paste — it transfers with Explorer's progress and no lag in typing meanwhile;
  - with one PC on an older version, files are not offered and text/images still cross.
- [ ] 8.3 Repeat 8.2's single-file and folder checks on Windows 10 22H2, deferred with the other two-PC checks if no machine is available.
