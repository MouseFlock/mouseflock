<p align="center">
  <img src="docs/brand/mark.svg" width="112" alt="MouseFlock">
</p>

<h1 align="center">MouseFlock</h1>

<p align="center">
  <b>One keyboard and mouse for all your Windows PCs.</b><br>
  Push the cursor off the edge of one screen and it appears on the next PC. Copy on one, paste on the other.
</p>

<p align="center">
  <a href="https://github.com/mouseflock/mouseflock/releases/latest">Download</a> ·
  <a href="#get-started">Get started</a> ·
  <a href="docs/known-issues.md">Known issues</a> ·
  <a href="SECURITY.md">Security</a>
</p>

---

MouseFlock is a software KVM: it shares one keyboard and mouse between PCs on your network, with no extra hardware. Each PC keeps its own screen.

There is nothing to set up. Install it on two PCs, type a short code to pair them, and push the cursor across. There are no server and client roles, no IP addresses to enter, and no screen grid to arrange.

## What it does

- **The cursor crosses screen edges** to the PC beside you. The keyboard follows the cursor. MouseFlock learns which PC is on which side from the way you move the cursor across, and a wrong guess can be undone.
- **Copy and paste between PCs:** text, images, and files and folders. Copy in Explorer on one PC, paste in a folder on the other. File bytes move only when you paste, so nothing large travels just because the cursor crossed.
- **Works where other tools stop:** UAC prompts, the lock screen and the sign-in screen, and windows running as administrator.
- **Built for different screens:** monitors with different scaling, and PCs with several monitors.
- **Private by design:** it works only on your local network, with no account and no cloud. Every connection is encrypted and tied to the PCs you paired.

## Get started

You need two or more PCs on the same network, each running **Windows 11** or **Windows 10 22H2**, on x64 or ARM64. Windows 10 should work, but this release hasn't been tested on it yet.

1. **Download** the setup program for your PCs from the [latest release](https://github.com/mouseflock/mouseflock/releases/latest): `MouseFlock-Setup-<version>-x64.exe` for most PCs, or `-arm64.exe` for ARM PCs such as Snapdragon laptops.
2. **Install it on each PC.** MouseFlock isn't code-signed yet, so Windows may say *"Windows protected your PC"*. Choose **More info**, then **Run anyway**. Setup opens the firewall for MouseFlock and starts it; there's nothing else to configure.
3. **Pair the PCs.** In MouseFlock on one PC, choose **Add a PC**, then **Show a code**. On the other PC, choose **Add a PC**, then **Enter a code**, and type the code it shows, such as `137-4829153`.
4. **Push the cursor** off the edge of the screen toward the other PC. It crosses, and your keyboard goes with it.

MouseFlock keeps running in the background and starts with Windows. The tray icon shows which PC you're controlling. Closing the window changes nothing.

> **If you ever get stuck** on the other PC's screen, for example when you can't see it, press **Scroll Lock twice** quickly. Your keyboard and mouse come back to the PC you're sitting at.

To uninstall, go to **Settings → Apps → Installed apps → MouseFlock**. Uninstalling removes everything, including the pairing.

## Security and privacy

MouseFlock sends your keystrokes over the network, so it is built to be audited:

- **Only your paired PCs can connect.** Pairing uses a one-time code with a PAKE (SPAKE2), so the code is never sent and can't be guessed offline. After that, every connection is QUIC with TLS 1.3, pinned to each PC's own key.
- **It stays on your local network.** It has no server, no account and no telemetry.
- **The background service never opens your files.** Copied files go straight from your user account on one PC to Explorer on the other.
- **The code that handles input is small and isolated.** It's written in Rust, `unsafe` code is confined to four Windows-specific crates, and the protocol and engine are kept dependency-light.

The [threat model](docs/threat-model.md) explains what MouseFlock defends against, and what it doesn't. To report a vulnerability, see [SECURITY.md](SECURITY.md). Please don't open a public issue.

## Status

MouseFlock is new; this is an early release. Known limitations are listed in [docs/known-issues.md](docs/known-issues.md). Bug reports and ideas are welcome in [issues](https://github.com/mouseflock/mouseflock/issues).

MouseFlock is Windows-only by design, and it doesn't do drag-and-drop between PCs, remote desktop, or connections over the internet.

## Building from source

MouseFlock is a Rust workspace with a small Tauri app for its window and tray. [AGENTS.md](AGENTS.md) describes the layout, the build and test commands, and the rules the code follows. [docs/design-brief.md](docs/design-brief.md) and the [architecture decisions](docs/adr/) explain why it's built this way, and [docs/protocol.md](docs/protocol.md) specifies the wire protocol.

```
cargo build --workspace
cargo test --workspace
installer\build.ps1 -Arch x64      # the setup program and MSI, in installer\out
```

Contributions are welcome. AI-assisted contributions are too, when they're tested; say which tool you used in the pull request.

## License

[Apache-2.0](LICENSE).
