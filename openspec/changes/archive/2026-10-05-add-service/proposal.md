# Proposal

## Why

Every piece of the input path now exists on its own, but nothing runs them together:
- the engine decides focus and routing;
- the transport pairs machines and carries engine messages and motion;
- the session agent captures and injects input over the `ipc` agent link.

ADR 0002 puts the engine and network in a SYSTEM service that launches the agents. Today `deskhop-service.exe` is an empty `main`. This change builds the service. At its end, the cursor crosses between two PCs, which the design brief sets as the milestone before any UI.

## What Changes

- **The service runs the engine.** One task owns the engine and connects it to the session agent's link and to the transport node:
  - agent input and screen reports go into the engine;
  - peer messages and peers going up or down go into the engine;
  - the engine's decisions go to the agent (capture mode, injection, cursor warp), to peers, to storage, and to the log.
  - Agent event times are converted to the service clock, as `add-ipc` specified.
- **Session agents.**
  - The service keeps one session agent running in the active console session, following logon, logoff and console session switches.
  - It restarts an agent that exits, with backoff.
  - Every launch goes through one function in `bin/service`, as AGENTS.md requires.
- **Agent pipes are protected.** This closes the threat model's open item from `add-ipc`:
  - each launch creates a new pipe with a random name, as the first and only instance, before the agent starts;
  - its ACL admits only SYSTEM and that session's user;
  - the service accepts only the process ID it launched.
- **Losing local input.** When the agent link ends (crash, logoff, session switch), the engine gets a new event:
  - if this machine was sending its input to a peer, the peer releases what it held and focus comes back here;
  - if a peer was controlling this machine, the peer gets its focus back.
  - Today the engine has no way to learn this, so a peer could stay controlled by input that will never arrive.
- **State on disk.**
  - Identity key, trust store, last-known addresses and learned layouts live in a data folder that only SYSTEM can open.
  - The service creates the folder, or takes it over safely if it already exists.
  - Each file is replaced atomically. A file that cannot be read is moved aside and logged, never silently trusted.
  - A first start creates the identity. The machine name is the computer name.
- **Power.** Before sleep the service says goodbye to every peer (`NodeHandle::goodbye`), so they release focus at once instead of after the 1-second timeout. On resume it reconnects.
- **Secure attention.** When a controlling peer asks for Ctrl-Alt-Del, the service calls `SendSAS`.
- **Two ways to run:**
  - **As a Windows service** under the Service Control Manager, logging to a file in the data folder. A dev script registers it from a copy in an admin-only folder (Program Files) and removes it again; the user runs that script.
  - **`deskhop-service --console [--data <dir>] [--port <n>]`** for development, as the logged-in user. It keeps its data in its own folder (per-user by default), launches the agent in the current session, and logs to the terminal.
    - It reads commands from the terminal: `pair`, `close`, `join <code>`, `peers`, `remove <peer>`, `undo`, `forget-layout`, `quit`.
    - It prints the code, pairing results, peers going up and down, focus changes, and learned sides with the Undo hint.
    - Until `add-onboarding` adds the UI, pairing happens here. The registered service runs, but cannot pair until then.
- **ADR 0006: a third `unsafe` crate, `win32-session`.** It is linked only by `bin/service` and wraps every Win32 call the service needs:
  - session and token queries;
  - launching a process as the session user;
  - pipes with a security descriptor, and the client process ID;
  - the data folder's owner and ACL;
  - `SendSAS`.
  - `cargo archcheck` enforces both rules: `unsafe` is allowed only there, and only `deskhop-service` depends on it.

## Capabilities

### New Capabilities
- `service-host`: the service process. It runs under the SCM or in console mode, owns the data folder and its files, creates the identity, connects the engine to the agent link and the node, says goodbye before sleep, requests secure attention, takes console commands, and shuts down.
- `session-agents`: which session gets an agent, how it is launched, how its pipe is protected and its process checked, and restarting it.

### Modified Capabilities
- `input-focus`: a new requirement, "Losing local input". When this machine's input agent ends, focus that depended on it is returned and held input is released.

## Impact

- **Crates touched:**
  - **`win32-session` (new):** `unsafe` allowed; `windows` features for remote desktop sessions, security, authorization, threading, environment and pipes; the SCM dispatcher entry if `windows-service`'s macro needs it (design D1).
  - **`bin/service`:** the host, supervisor, store, console and SCM glue. It stays `#![forbid(unsafe_code)]`. New dependencies: `engine`, `model`, `win32-session`, `windows-service`, `tokio`, `serde`, `serde_json` and `rand_core`; all except `win32-session` are already workspace dependencies.
  - **`engine`:** `Event::LocalInputLost`, with replay tests. No new dependency.
  - **`tools/archcheck`:** the new rules.
  - **`transport`:** a bug fix found during apply. A completed pairing recorded the new peer's address without reporting it, so the inviter never stored where its peer was, against peer-transport's "State for the service to store". The fix adds test `addresses_reported_after_pairing`. No behavior change otherwise.
  - **No change:** `ipc` (no message changes, so `IPC_VERSION` stays 1), `pairing`, `bin/agent`.
- **Dependency rules hold:**
  - `engine` still reaches no `win32-*`, `transport` or `ipc`.
  - Only `deskhop-agent` links `win32-input` and `win32-clipboard`, so the service still never touches input.
  - Only `deskhop-service` links `win32-session`.
  - `#![forbid(unsafe_code)]` stays everywhere else.
  - `proto`, `model` and `engine` gain no dependencies.
  - The engine's new event has no Windows types, and its tests run on Linux.
- **Docs:**
  - ADR 0006, status Proposed.
  - AGENTS.md: repo layout, dependency rules, unsafe list and commands.
  - `docs/design-brief.md`: "`unsafe` is confined to three crates".
  - `docs/threat-model.md`: closes the pipe item, and adds the data folder, console mode and secure attention.
  - CI's ARM64 check adds `win32-session` and `deskhop-service`.
- **System changes are the user's.** Registering the service, copying it into Program Files and setting the policy that allows `SendSAS` are done by the dev script, which the user runs as administrator. `add-installer` takes them over later.
- **No grid, no server/client toggle, no IP address field.** Peers are named by machine name or key prefix in console commands, never by address.
- **Later:**
  - `add-onboarding`: the UI pipe and UI pairing.
  - The winlogon agent: input on the secure desktop.
  - Sessions other than the console (RDP).
  - `add-installer`.
