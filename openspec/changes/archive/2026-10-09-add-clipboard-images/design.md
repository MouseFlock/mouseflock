# Design

## Context

- **The text path today (archived add-text-clipboard):**
  - **Agent:** `win32-clipboard`'s listener thread reports each settled copy (100 ms) as text, or not shared with a reason: `0` private, `1` no text, `2` text too large. It sets text without reporting it back.
  - **IPC:** runs of `ClipboardText(len)` or `SetClipboard(len)`, then chunks (`0x0016`–`0x0018`, `0x0027`–`0x0028`).
  - **Service:** `bin/service/src/clipboard.rs` holds the `Tracker`, with the held copy as `Arc<str>`, offers, the newest copy, and `FETCH_WAIT` of 10 s measured from the request.
  - **Transport:** `crates/transport/src/conn.rs` writes one unidirectional stream per transfer at priority -1, and reads one at a time through `proto::clipboard::TransferReader`.
  - **Peer protocol:** `ClipboardOffer`/`ClipboardRequest` (`0x0011`/`0x0012`, ID only) on the control stream; `ClipboardData` (`0x0013`, id u64 and len u32) and `ClipboardChunk` (`0x0014`) on clipboard streams.
- **Versions.**
  - Peer and IPC protocols are both at version 1, and `Hello` negotiates the highest common version (`proto::hello::negotiate`). IPC decoding already takes the agreed version.
  - Version 1 has now shipped: the 0.1.0 setup program is installed on the user's two PCs. So, unlike every earlier change, this one adds a version rather than filling in version 1.
- **Free type numbers:** peer `0x0015`; IPC `0x001A` (agent to service) and `0x0029` (service to agent).
- **Fuzzing:** `tools/fuzz` has `decode_stream`, `decode_datagram` and `decode_ipc`.

## Goals / Non-Goals

**Goals:**
- Every requirement in the deltas to `clipboard`, `agent-link`, `wire-protocol` and `ipc-protocol`.
- Reuse the text path wherever possible: the same offers, requests, tracker and streams, with a kind attached.
- Keep image parsing out of the SYSTEM service.

**Non-Goals:**
- Files, rich text and HTML, and several formats of one copy.
- Delayed rendering: setting the bitmap only when an app asks for it. Decoding up front at 64 megapixels at most is affordable, and it keeps the agent simple (D5).
- Any UI.

## Decisions

### D1. A kind on the transfer, not on the offer

`ClipboardOffer` and `ClipboardRequest` stay ID-only and unchanged, so version 1 peers keep understanding them. The kind travels in the stream's header:
- **text:** `ClipboardData` (`0x0013`), unchanged;
- **image:** the new `ClipboardImageData` (`0x0015`, version 2), with the same 12-byte payload of `id: u64`, `len: u32`.

The receiver learns the kind when the stream starts, which is the only point it needs it, to pass to the agent. `TransferReader` takes the agreed version and accepts either header. It returns the kind in `TransferStep::Started`, and applies each kind's length limit: 4 MiB for text, 32 MiB for an image. For an image it also checks the first eight bytes against the PNG signature, across chunk boundaries.

Alternative: put the kind in the offer, as a new `ClipboardImageOffer`. That needs a second offer type, and a version 1 peer would still need filtering. Rejected as more surface for no gain.

### D2. Offer images only on version 2 connections

The node records each connection's agreed version when its `Hello` exchange completes. `offer_clipboard(peer, id, kind)` and `send_clipboard(peer, id, contents)` take the kind, and the node sends an image offer or transfer only on a connection at version 2 or later, dropping it otherwise. The service offers every copy to every up member, as before, and needs no version bookkeeping.

*Changed while implementing:* the proposal had `Event::PeerUp` carry the version and the service's `Tracker` keep it. Deciding in the node is simpler, and it stays right when a connection is replaced, or comes back within the limbo window without a new `PeerUp`. The spec's behavior is the same: a version 1 member receives no offer.

A version 1 member never learns of the image copy. So its newest copy stays the previous one, and if it requests that earlier ID, the owner answers with nothing (the stale-request rule). That's the accepted behavior when mixing in 0.1.0.

The connection task also refuses an outgoing image on a version 1 connection, as a second guard. Tests stand in for a 0.1.0 peer by narrowing the node's advertised range (`Config::hello`).

### D3. The held copy carries its kind

`Tracker`'s held and received copies become `Clip { kind: Text(Arc<str>) | Image(Arc<[u8]>) }`. The offer, newest-copy and stale-request logic is unchanged and stays kind-blind. The kind decides only which IPC run carries the copy to the agent, and which stream header carries it to a peer.

The log line becomes `clipboard: offered image, 1234567 bytes`, so the kind and length are logged, never the contents.

### D4. Progress-based fetch timeout

`FETCH_WAIT` stays at 10 s, but is measured from the last byte received rather than from the request. The transport reports progress with each chunk (`Internal::ClipboardProgress(id)`, coalesced to at most one report per second), and the service's `Tracker` moves the deadline forward. Text transfers behave as before: they finish well inside 10 s.

### D5. The agent: reading, encoding, decoding and setting images

- **Reading a copy**, after the privacy check, which already runs first and reads no contents. In order:
  1. **Text** (`CF_UNICODETEXT`): shared as text, as today.
  2. **The registered `PNG` format**, if present. Its bytes are used as they are, after a header check: a valid signature, `IHDR` dimensions within 64 megapixels, at most 32 MiB.
  3. **`CF_DIBV5`, then `CF_DIB`.** Encoded to PNG: 32 bpp with alpha when the DIB says so (`BI_BITFIELDS` with an alpha mask, or `CF_DIBV5`'s `bV5AlphaMask`), otherwise 24 bpp. Bottom-up and top-down DIBs are both handled. Supported depths are 24 and 32 bpp. Other depths (1, 4, 8 and 16 bpp, and compressed DIBs) are reported as not shared (`1`) rather than half-supported; screenshots and browsers produce 32 or 24 bpp.
  4. **Otherwise** not shared (`1`, now meaning "no text or image").
- **Setting a received image:** the agent decodes the PNG to RGBA with the `png` crate, refusing anything over 64 megapixels before allocating. It opens the clipboard and sets:
  - the registered `PNG` format, as the received bytes;
  - `CF_DIBV5`, 32 bpp bottom-up with an alpha mask, in sRGB. Windows synthesizes `CF_DIB` and `CF_BITMAP` from it. *Changed while implementing:* the proposal said top-down; bottom-up is what every app reads, while some mishandle a negative-height bitmap on the clipboard.

  A decode failure leaves the clipboard unchanged and is logged by the agent. The link stays up.
- **Pure functions:** DIB to PNG and PNG to DIB live in `win32-clipboard/src/image.rs`, apart from the clipboard calls, and are unit-tested: round trips, alpha, orientation, the limits, and corrupt input.

### D6. PNG via the `png` crate, not Windows Imaging Component

The agent decodes PNGs that came from another machine, so the decoder parses untrusted input.
- The pure-Rust `png` crate is memory-safe, fuzzable in `tools/fuzz`, and has a small dependency tree (MIT/Apache-2.0). `win32-clipboard` gains it as a dependency. It is linked only by `deskhop-agent`.
- Windows Imaging Component would add no dependency, but means COM, `unsafe` FFI and an unauditable C++ decoder on untrusted bytes.

Chose `png`.

### D7. IPC version 2

- **New messages:**
  - `ClipboardImage` (`0x001A`, session agent to service, `len: u32` up to 33,554,432);
  - `SetClipboardImage` (`0x0029`, service to session agent, same layout).

  Both are followed by the existing chunk messages, whose run rules now cover either start message.
- **No new reason codes:** reason `1` reads "no text or image", and `2` reads "too large" (text over 4 MiB, or an image over its limits). On a version 1 link the agent reports an image copy as `1`, as 0.1.0 does.
- **The run reader checks** the PNG signature of an image run before handing it on. The service checks only that and the length (spec "The service never decodes images").

### D8. Peer protocol version 2

- `PROTOCOL_VERSION` becomes 2, and `MIN_PROTOCOL_VERSION` stays 1.
- The registry gains `0x0015` `ClipboardImageData` (Transfer, version 2).
- `version_1_defines_exactly_these_types` stays as it is, and a new `version_2_adds_only_clipboard_image_data` test joins it.
- `docs/protocol.md` gets a version history row for peer 2 and IPC 2, and the registry tables gain the "version" column entries.

### D9. Threat model

`docs/threat-model.md`'s clipboard section gains a section on untrusted image data:
- **Who parses it:** a member can send any bytes labelled as a PNG, so only the agent decodes them, as the user and with a memory-safe decoder.
- **Limits:** dimensions are capped before allocation, and transfers at 32 MiB.
- **The service:** it never parses an image; it checks the signature and length only.
- **Memory:** in the worst case, the service holds one 32 MiB held copy plus one incoming transfer per connection.

## Risks / Trade-offs

- **[A 32 MiB transfer on slow Wi-Fi competes with motion for bandwidth]** → The stream runs at priority -1, and QUIC paces datagrams and the control stream independently of it. The spec's "typing during a large transfer" scenario is checked on two PCs.
- **[Text wins over images]** → An Office copy that a user wanted to paste as a picture arrives as text. That matches today's behavior, and sharing several formats is out of scope. The rule is stated in the spec.
- **[Uncommon DIB depths aren't shared]** → They are rare on modern Windows, and are reported as not shared rather than garbled.
- **[A mixed desk with 0.1.0 PCs]** → Image copies don't reach those PCs. Upgrading them fixes it. The installer upgrades in place and keeps the pairing.
- **[Service memory]** → The service holds copies of up to 32 MiB, bounded as in D9.

## Migration Plan

- **Rollout:** an upgrade installs the new service and agent together. A PC still on 0.1.0 keeps working at version 1, with text only.
- **Rollback:** reinstall the older version. Version negotiation handles the mix.

## Open Questions

None.
