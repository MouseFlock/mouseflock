# Tasks

## 1. Peer protocol version 2 (`proto`)

- [x] 1.1 Bump `PROTOCOL_VERSION` to 2, keeping `MIN_PROTOCOL_VERSION` at 1. Add `0x0015` `ClipboardImageData` (Transfer, version 2) to the registry, and `MAX_IMAGE_LEN` (33,554,432) with the PNG signature constant (D8). Verify with unit tests:
  - `version_1_defines_exactly_these_types` is unchanged and passes;
  - a new `version_2_adds_only_clipboard_image_data` test passes;
  - `Hello` from this release encodes as `01 00 02 00`;
  - negotiation with a 1–1 peer agrees on 1.
- [x] 1.2 Extend `TransferReader` to take the agreed version and accept `ClipboardImageData`. `TransferStep::Started` reports the kind, each kind gets its own limit, and an image's first eight bytes are checked against the PNG signature even when split across chunks (D1). Add `encode_image_data`. Verify with unit tests:
  - an image stream of exactly 32 MiB is accepted, and 40,000,000 bytes is refused from the header alone;
  - `ClipboardImageData` at version 1 is a protocol error;
  - a wrong signature, including one split 3+5 across two chunks, is a protocol error;
  - text streams behave exactly as before.
- [x] 1.3 Extend `tools/fuzz`'s `decode_stream` target to drive `TransferReader` at versions 1 and 2. Verify that it builds and runs for 60 seconds without findings.

## 2. Transport (`transport`)

- [x] 2.1 Record each connection's agreed version in the node and send image offers and transfers only at version 2 or later (D2 as changed), carry the clipboard kind through sending and receiving, with progress coalesced to one report per second (D2, D4). Refuse to send an image on a version 1 connection. Verify with loopback tests:
  - a 1 MiB image round-trips between two nodes at version 2;
  - a node whose `Hello` advertises 1–1 agrees version 1 with a 1–2 node; pairing and a text transfer still work, and sending an image to it is refused locally without closing the connection;
  - a 20 MiB transfer reports progress at most once per second;
  - while it runs, engine messages on the control stream are still delivered within 50 ms.

## 3. IPC version 2 (`ipc`)

- [x] 3.1 Bump `IPC_VERSION` to 2, keeping the minimum at 1. Add `ClipboardImage` (`0x001A`) and `SetClipboardImage` (`0x0029`) with the image run rules: version 2 only, at most 33,554,432 bytes, the PNG signature checked across chunks. Reasons `1` and `2` now read "no text or image" and "too large" (D7). Verify with link tests:
  - image runs round-trip in both directions;
  - `ClipboardImage` on a version 1 link closes it;
  - a chunk after `ClipboardImage` without a run, or a bad signature, closes it;
  - a version 1 agent links with a version 2 service at version 1, and text still works;
  - the never-panic test covers the new types.
- [x] 3.2 Update `docs/protocol.md`:
  - the peer registry row for `0x0015` and the IPC rows for `0x001A` and `0x0029`, with version 2;
  - the clipboard runs paragraph and the reason codes;
  - the clipboard stream section's image header and limits;
  - version history rows for peer 2 and IPC 2.

  Verify that every row matches the registries in `proto` and `ipc`.
- [x] 3.3 Extend `tools/fuzz`'s `decode_ipc` target to cover the new types. Verify that it runs for 60 seconds without findings.

## 4. Images on the clipboard (`win32-clipboard`, `bin/agent`)

- [x] 4.1 Add `png` to `win32-clipboard` (D6) and write `image.rs`, pure functions with no clipboard calls:
  - DIB or DIBV5 at 24/32 bpp, top-down or bottom-up, with or without alpha, to PNG;
  - PNG to a 32 bpp DIBV5, refusing over 64 megapixels before allocating (bottom-up, as changed in D5);
  - a PNG header check (signature, dimensions, length).

  Verify with unit tests:
  - round trips preserve pixels and alpha;
  - a bottom-up DIB comes out the right way up;
  - 8 bpp and RLE DIBs are refused;
  - a 10 KB PNG claiming 100,000 × 100,000 is refused without a large allocation;
  - truncated and corrupt PNGs return errors, never panics.

  Add a fuzz target, `decode_png_to_dib`, and run it for 60 seconds without findings. Check that `png` and its dependencies are MIT and/or Apache-2.0.
- [x] 4.2 In the listener, read a copy in D5's order: privacy, then text, then the `PNG` format, then `CF_DIBV5`/`CF_DIB`. Report images with a kind, and set received images as `PNG` plus `CF_DIBV5`, never reporting them back. Verify:
  - unit tests of the order decision, given which formats are present;
  - ignored desktop tests on this PC:
    - an image put on the clipboard as `CF_DIB` is reported as a PNG of the same pixels;
    - one put on as `PNG` is reported byte for byte;
    - an image set by the listener can be read back as `CF_DIB`, and is not reported;
  - `cargo clippy -p win32-clipboard --all-targets -- -D warnings` is clean, and every new `unsafe` block has a `// SAFETY:` comment.
- [x] 4.3 In `deskhop-agent`, send `ClipboardImage` runs on version 2 links, and report images as reason `1` on version 1 links. Handle `SetClipboardImage` by decoding and setting, leaving the clipboard unchanged on failure. Verify with the agent's link tests over a pipe pair:
  - an image copy reaches the service end as a run at version 2, and as `ClipboardNotShared(1)` at version 1;
  - a corrupt `SetClipboardImage` leaves the link up.

  Done:
  - **Desktop tests with the real agent:**
    - a bitmap copied through .NET arrives as a 5 × 2 PNG run;
    - an image set by the service reads back through .NET as 4 × 3 and is not reported;
    - a corrupt image is logged, and a later text set still works.
  - **The version 1 case** is covered by the IPC link test `agent_on_a_version_1_link_reports_images_as_no_text`. It drives the same `ServiceHandle::send_clipboard_image` the agent calls, against a service end that speaks only version 1.

## 5. The service (`bin/service`)

- [x] 5.1 Make the `Tracker`'s copies carry their kind (D3). Move the fetch deadline on progress, and drop a copy that arrives after its fetch gave up (D4). Offering images only to version 2 members is the node's job (D2 as changed), checked in 2.1 and in 5.2's mixed-version host test. Log kind and length only. Verify with `clipboard.rs` unit tests:
  - a copy that keeps making progress for 25 s is set, and one with 10 s of silence is dropped;
  - the newest copy across kinds wins;
  - the stale request still gets nothing.
- [x] 5.2 Wire image runs between the agent link, the `Tracker` and the transport in `host.rs`, passing image bytes unparsed. Verify with host tests over loopback (the add-text-clipboard style):
  - **screenshot on A, cross to B:** B's agent end receives `SetClipboardImage` with the same bytes;
  - **text after an image:** C fetches only the text;
  - **image not echoed;**
  - **member running 0.1.0:** a node at version 1 receives no offer, and its connection stays up;
  - neither service log contains any bytes of the image.

## 6. Docs

- [x] 6.1 Add the "Untrusted image data" part to `docs/threat-model.md`'s clipboard section (D9), and update its status line. Update the clipboard spec's Purpose to say "text and images" when the change is archived. Verify by reading the threat model against `specs/clipboard` and D5–D7.

  Done: the threat model has the "Untrusted image data" part and two new residual risks, and its status line names images. The Purpose update for `openspec/specs/clipboard` is done in the archive step, when that spec is synced.
- [ ] 6.2 Update `docs/known-issues.md` only if the 7.2 check finds a limitation users will hit. Otherwise record "no change" here.

## 7. Integration

- [ ] 7.1 Run the full checks, and verify that each passes:
  - `cargo fmt --all --check`, `cargo archcheck`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`;
  - the Linux set, including `proto` and `ipc`;
  - the ARM64 check;
  - `npm test`;
  - the CI run, including the `installer` job.
- [ ] 7.2 Manual check with the user on both PCs. Build the setup program with `installer\build.ps1 -Arch x64 -Version 0.2.0` and install it over 0.1.0 on both PCs:
  - **Upgrade:** both PCs report each other up again without pairing (add-installer 6.3, upgrade part).
  - **Screenshot:** Win+Shift+S on A, cross to B, paste in Paint and in a browser page that accepts images.
  - **Copy image:** "Copy image" in a browser on B, cross to A, and paste.
  - **Text still wins:** copy cells in Excel or a table in Word on A, cross to B, and paste in Notepad. The text arrives.
  - **Large image:** a 4K screenshot. Type on B immediately while it transfers: no lag, and the image pastes when it arrives.
  - **Mixed versions,** if practical: with one PC back on 0.1.0, text still crosses and images don't, with no errors in either log.
