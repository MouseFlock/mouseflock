# Security policy

MouseFlock types keystrokes it receives over the network, and its service runs as SYSTEM. Security reports are taken seriously and are welcome.

## Reporting a vulnerability

**Please report vulnerabilities privately, not in a public issue.**

Use GitHub's private reporting: on this repository's **Security** tab, choose **Report a vulnerability**. Only the maintainers can see the report, and you can follow it there.

Please include:

- what an attacker can do, and from where (any host on the LAN, a paired PC, a local user on the same PC, …);
- the MouseFlock version and Windows version;
- steps or a proof of concept, if you have one.

MouseFlock is a small open-source project, so responses are best effort. A report will be acknowledged as soon as possible, kept private while a fix is prepared, and credited in the release notes unless you'd rather stay anonymous.

## Supported versions

Only the latest release gets security fixes. MouseFlock is early, and fixes ship as a new release rather than as patches to older ones.

## Scope

In scope is anything that breaks what the [threat model](docs/threat-model.md) says MouseFlock defends against. For example:

- a host on the network that isn't paired gets to inject input, read clipboard contents or keystrokes, or join the group of paired PCs;
- the pairing code can be guessed or brute-forced faster than the threat model allows;
- a local, non-admin user on one PC gains SYSTEM, or controls another user's session, through MouseFlock's service, its agents or its pipes;
- a received file is written outside the folder the user pasted into, or a hostile manifest exhausts the receiving PC;
- malformed network or pipe data crashes the service, or makes it misbehave.

Out of scope, as the threat model describes:

- A paired PC typing on, or reading the clipboard of, the PCs it's paired with. That is what pairing grants. Removing the PC ends it.
- Attacks that need administrator rights, or physical access, on the PC already.
- Builds you made yourself with changes, and console mode, which is for development only.
