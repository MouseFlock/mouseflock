# Tools

| Tool | Status | Purpose |
|---|---|---|
| `archcheck/` | Ready | Enforces the dependency and `unsafe` rules in AGENTS.md. `cargo archcheck`. |
| `bootstrap.cmd` | Ready | Runs `bootstrap.ps1` from an elevated terminal. Installs the Windows toolchain: Rust, MSVC Build Tools, Windows SDK, Node, WiX. |
| `fuzz/` | Ready | cargo-fuzz targets for `proto` stream and datagram decoding. Run nightly in CI (`.github/workflows/fuzz.yml`). Locally: `cargo +nightly fuzz run --fuzz-dir tools/fuzz decode_stream -- -max_total_time=60` (needs `cargo install cargo-fuzz`; on Windows, the MSVC `bin/Hostx64/x64` folder holding `clang_rt.asan_dynamic-x86_64.dll` must be on `PATH`). |
| packet capture | Planned | Decode captured MouseFlock traffic. |
| event replay | Planned | Feed recorded `model` event sequences through `engine`. |
