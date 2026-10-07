# Proposal

## Why

ADR 0002 exists because every lightweight software KVM breaks at the same moments: a UAC prompt, the lock screen, the logon screen. Windows shows these on the secure desktop (`winsta0\Winlogon`), where the session agent's hooks see nothing and its `SendInput` reaches nothing. Today DeskHop breaks at exactly those moments too. `deskhop-agent.exe --winlogon` exits at once, and the service never starts it. This is the last engine piece of the MVP's "reliable at the hard moments" promise.

## What Changes

- **A winlogon agent on the secure desktop.** The service keeps one `deskhop-agent.exe --winlogon --pipe <name>` running in the console session.
  - It runs as SYSTEM on `winsta0\Winlogon`, so it can capture and inject there.
  - **It runs whether or not a user is logged on**, as agreed with the user, so a freshly booted PC can be logged on to from another PC.
  - It starts with every privilege removed except the one every process needs (`SeChangeNotifyPrivilege`), so it is SYSTEM with nothing extra.
  - Console mode runs none, because it cannot launch as SYSTEM. It logs that once.
- **The same supervision as the session agent:**
  - a fresh pipe per launch, its ACL limited to SYSTEM;
  - only the launched process is accepted;
  - a 10-second link deadline;
  - restart with backoff, ended and terminated the same way.
  The two agents are supervised independently.
- **Whichever desktop has the input gets it.**
  - The winlogon agent reports whether its desktop is the input desktop, when it links and on every change.
  - While its desktop is the input desktop, the service sends injection and warps to it, and takes input and the screen from it. Otherwise these go to the session agent, as now.
  - Both agents always get the current capture mode, so physical input is withheld or passed the same way on either desktop.
- **What the user sees:**
  - With the cursor on a PC, they can type into its UAC prompt, unlock its lock screen, or log on at its logon screen.
  - If a PC they are controlling another PC from shows a UAC prompt or locks, their keyboard and mouse keep driving the other PC until they bring the cursor back.
- **No clipboard on the secure desktop.** The winlogon agent never reads or sets the clipboard. A clipboard message from it closes its link.
- **Agent link (IPC version 1, not shipped, so no bump):**
  - one new agent → service message, `InputDesktop`, sent only by the winlogon agent;
  - the service end of each agent pipe accepts only the role of the agent it launched there;
  - clipboard messages travel only on a session agent's link.
- **Docs:**
  - `docs/threat-model.md` fills in the secure desktop: a SYSTEM agent in the user's session, what a paired member can now do (answer UAC prompts, type at the lock and logon screens), and what the agent never does.
  - `docs/protocol.md` gets the new message and the per-role rules.
  - AGENTS.md shows the winlogon agent's command line.
- **Out of scope:**
  - **Elevated apps on the user's desktop**, as agreed with the user. After a UAC prompt is approved, the elevated window runs at high integrity on the normal desktop, where the session agent cannot inject (UIPI) and its hooks may not see input. This is a follow-up change, probably a `uiAccess` session agent that ships with the signed installer.
  - Desktops other than `Default` and `Winlogon`, such as the secure screen saver and apps' private desktops.
  - RDP sessions.
  - Any off switch or policy for secure desktop control.

## Capabilities

### New Capabilities
- `secure-desktop`: which agent carries this machine's input when the secure desktop is the input desktop. It covers the input desktop reports, routing injection, warps, captured input and the screen to the agent on the input desktop, the capture mode on both desktops, what happens when that agent's link ends, and no clipboard there.

### Modified Capabilities
- `session-agents`: the service also keeps a winlogon agent in the console session, logged on or not. It launches it as SYSTEM with privileges removed, on the secure desktop, behind a SYSTEM-only pipe, under the same supervision rules. Console mode runs none.
- `agent-link`: the winlogon agent's command line and role. It reports `InputDesktop` and never starts the clipboard. The service accepts only the launched agent's role on each pipe, and clipboard messages only from the session agent.
- `service-host`: the engine's decisions for "the agent" go to the agent on the input desktop, and an orderly stop ends every agent link.

## Impact

- **Crates touched:**
  - **`win32-session`** (service only): a SYSTEM token for a session, with privileges removed; launching onto a named desktop.
  - **`win32-input`** (agent only): a watcher that reports whether the thread's own desktop is the input desktop.
  - **`ipc`:**
    - the `InputDesktop` message;
    - the expected role on each end of an agent link;
    - role checks for clipboard and `InputDesktop` messages.
  - **`bin/agent`:** the `--winlogon --pipe` mode. It shares the session agent's link loop, with no clipboard and with the desktop watcher.
  - **`bin/service`:**
    - `launch::agent` takes the agent kind;
    - the supervisor runs two plans;
    - the host holds two agent links and picks the active one;
    - unit and host tests.
  - **`tools/fuzz`:** `decode_ipc` covers `InputDesktop`.
  - **Docs:** `docs/protocol.md`, `docs/threat-model.md`, AGENTS.md.
  - **No change:** `engine`, `model`, `proto`, `transport`, `pairing`, `policy`, `win32-clipboard`, `win32-shell`, `apps/desktop`.
- **Dependency rules hold:**
  - Only `deskhop-agent` links `win32-input`; the desktop watcher lives there.
  - Only `deskhop-service` links `win32-session`; the token and launch calls live there.
  - `engine` is untouched. The host chooses the agent and feeds the engine the events it already handles (`ScreenChanged`, `LocalInputLost`).
  - No new `unsafe` crate, no new dependency in `proto`, `model` or `engine`, and no new ADR. ADR 0002 already decides the winlogon agent, and ADR 0006 already puts its launch in `win32-session`.
  - Every launch still goes through `launch::agent`.
- **No grid, no server/client toggle, no IP address field.**
- **Testing:** the parts that need SYSTEM (the token, launching onto `Winlogon`, hooks and injection on the secure desktop) can only be checked through the registered service (`tools\dev\service.ps1`). The user runs those checks on one PC. Two-PC checks join the deferred list.
