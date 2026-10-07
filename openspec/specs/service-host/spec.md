# service-host Specification

## Purpose

The DeskHop service process. It runs under the Service Control Manager or in console mode, keeps this machine's identity and learned state on disk, connects the engine to the session agent and to peers, and handles sleep, secure attention and shutdown.

## Requirements

### Requirement: Service and console modes
`deskhop-service.exe` started by the Service Control Manager SHALL run as the service named `DeskHop`. Started with `--console`, it SHALL run as the current user in the current session. Console mode SHALL keep its data under the user's local application data, or in the folder given with `--data`, and SHALL never touch the service's data folder. `--port` SHALL set its UDP port. If the port is taken, it SHALL exit with an error naming the port.

#### Scenario: Console mode's own folder
- **WHEN** the service is registered and a user starts `deskhop-service --console`
- **THEN** console mode uses a data folder under that user's profile and leaves the service's files untouched

#### Scenario: Two console instances
- **WHEN** two console instances run on one machine with different `--data` folders and ports
- **THEN** each keeps its own identity and trust store, and each finds the other through discovery

#### Scenario: Port taken
- **WHEN** console mode starts on a port another process holds
- **THEN** it exits with a nonzero code and an error naming the port

### Requirement: Protected data folder
In service mode, the data folder SHALL be owned by SYSTEM, with an ACL that grants access only to SYSTEM and allows no inheritance from its parent. This SHALL hold before any file in the folder is read. If the folder already exists with an owner other than SYSTEM or the Administrators group, the service SHALL move it aside under a new name, log that, and create a new folder.

#### Scenario: Folder planted by a user
- **WHEN** a standard user created the data folder and put an identity file in it before the service first started
- **THEN** the service moves that folder aside without reading the file, and starts with a new identity in a new folder

#### Scenario: Normal restart
- **WHEN** the service restarts and its folder is owned by SYSTEM
- **THEN** it uses the folder and its files as they are

### Requirement: Identity on first start
When the data folder holds no identity, the service SHALL create one and store it before starting the network. Every later start SHALL use the stored identity. The machine name SHALL be the computer name, cut at a character boundary to at most 63 bytes.

#### Scenario: First start
- **WHEN** the service starts with an empty data folder
- **THEN** it creates and stores an identity, and its peer identity is the same after a restart

### Requirement: Learned state survives restarts
The service SHALL store the trust store file whenever membership changes, the last-known peer addresses whenever they change, and the learned layouts whenever they change. At start it SHALL load all three, give the trust store and addresses to the network, and give the layouts to the engine before the first agent report.

#### Scenario: Restart keeps the desk and layout
- **WHEN** A and B are paired, A learned that B is to its right, and A's service restarts
- **THEN** A reconnects to B without pairing again, and pushing past A's right edge crosses to B without learning again

### Requirement: Safe file replacement
Every stored file SHALL be replaced atomically, so a crash or power loss leaves either the old contents or the new ones. A stored file that cannot be read or parsed SHALL be moved aside under a new name and logged. The service SHALL then start as if that file were absent. It SHALL NOT stop, and SHALL NOT use part of the file.

#### Scenario: Damaged layouts file
- **WHEN** the layouts file holds bytes that do not parse
- **THEN** the service logs it, moves the file aside, starts with no learned layouts, and keeps the desk

#### Scenario: Power lost while writing
- **WHEN** power is lost while the service writes the trust store
- **THEN** at the next start the trust store holds either the previous membership or the new one

### Requirement: The engine drives the agent and peers
The service SHALL feed the engine with the active agent's input in service-clock time, its screen reports, peer messages, and peers coming up or going down. It SHALL carry out every decision: capture mode to every linked agent, injection and warp to the active agent, messages to peers, and layouts to storage. The active agent is the one on the input desktop (`secure-desktop`). A decision for an agent while none is linked SHALL be dropped.

#### Scenario: The cursor crosses
- **WHEN** A and B are paired and up, A learned that B is to its right, and the user pushes A's cursor through A's right edge
- **THEN** A's agent withholds all physical input, and B's agent receives a warp to B's left edge followed by the user's injected movement

#### Scenario: Peer goes down while controlling
- **WHEN** A controls B and A's service stops
- **THEN** B's engine learns that A is down and B's agent stops withholding the mouse

### Requirement: A new agent link gets the current capture mode
When an agent link comes up, the service SHALL first send it the capture mode for the engine's current focus: pass all when focus is local, withhold all when forwarding, and withhold mouse when controlled. Only then SHALL other commands follow.

#### Scenario: Agent links while controlled
- **WHEN** A takes control of this machine while it has no agent, for example between agent restarts, and an agent then links
- **THEN** the new agent's first command withholds the mouse

#### Scenario: Agent links with local focus
- **WHEN** focus is local and an agent links
- **THEN** the new agent's first command passes all input

### Requirement: Goodbye before sleep
When Windows reports that this machine is going to sleep, the service SHALL say goodbye to every peer before acknowledging, waiting at most 1 second. When Windows reports the machine has resumed, the service SHALL reconnect to its peers.

#### Scenario: Laptop lid closed while controlling a peer
- **WHEN** A's input has focus on B and A goes to sleep
- **THEN** B reports A down as having said goodbye at once, and B's input works locally

#### Scenario: Resume
- **WHEN** A resumes from sleep
- **THEN** A and B report each other up again without pairing

### Requirement: Secure attention for a controlling peer
When the engine asks to show the secure attention screen, the registered service SHALL ask Windows for it with `SendSAS`. It SHALL first check that the software secure attention policy allows services; if not, it SHALL log that instead. In console mode it SHALL log that secure attention needs the registered service. In every case the service SHALL carry on.

#### Scenario: Ctrl+Alt+End from the controller
- **WHEN** A controls B, where the registered service runs and policy allows services to generate secure attention, and the user presses Ctrl+Alt+End on A
- **THEN** B shows its Ctrl-Alt-Del screen

#### Scenario: Policy forbids it
- **WHEN** the same happens and B's policy does not allow services to generate secure attention
- **THEN** B logs which policy value is missing, and stays controlled by A

#### Scenario: Console mode
- **WHEN** the same happens with B in console mode
- **THEN** B logs that secure attention needs the registered service, and stays controlled by A

### Requirement: Console commands
In console mode the service SHALL read one command per line from the terminal: `pair`, `close`, `join <code>`, `peers`, `remove <peer>`, `undo`, `forget-layout` and `quit`. A peer SHALL be named by its machine name or a prefix of its key in hex. An unknown command or an ambiguous peer SHALL print an error and change nothing.

#### Scenario: Pairing two machines from the console
- **WHEN** the user types `pair` on A, which prints a code, and types `join` with that code on B
- **THEN** both print that they paired, naming the other machine, and `peers` on each lists the other as up

#### Scenario: Ambiguous peer
- **WHEN** two members' keys start with `ab` and the user types `remove ab`
- **THEN** the console says the name is ambiguous and removes nothing

### Requirement: Console reports
In console mode the service SHALL print the pairing code when pairing opens, and why pairing closes. It SHALL also print pairing and join results, peers going up and down, focus changes, and each learned side with how long `undo` remains possible.

#### Scenario: Learned side
- **WHEN** the user pushes through A's right edge and A learns that B is there
- **THEN** A's console prints that B was learned to the right and that `undo` takes it back within 10 seconds

### Requirement: Orderly stop
On a stop request (the Service Control Manager stopping the service, system shutdown, `quit`, or Ctrl+C in console mode), the service SHALL say goodbye to every peer, end every agent link, and finish pending writes before exiting. Ending the links SHALL make every agent stop withholding input.

#### Scenario: Stopped while forwarding
- **WHEN** A's input has focus on B and A's service is stopped
- **THEN** B reports A down as having said goodbye and releases what A held, A's agents exit, and A's keyboard and mouse work locally

#### Scenario: Stopped at the lock screen
- **WHEN** A is locked, its input has focus on B, and A's service is stopped
- **THEN** A's winlogon agent stops withholding and exits, and the user can type A's password at A's own keyboard

### Requirement: Logs
In service mode the service SHALL log to a file in its data folder that the Administrators group can read. When that file reaches 1 MB, it SHALL be renamed to keep exactly one previous file, and a new file SHALL be started. In console mode it SHALL log to the terminal. Logs SHALL NOT contain pairing codes, keys or input contents.

#### Scenario: Log rotation
- **WHEN** the log file reaches 1 MB
- **THEN** it becomes the previous log, replacing any older one, and logging continues in a new file

#### Scenario: An administrator reads the log
- **WHEN** an administrator in an elevated shell opens the service log by its path
- **THEN** they can read it, while the data folder's other files stay closed to them

#### Scenario: No keystrokes in logs
- **WHEN** the user types while their input is forwarded to a peer
- **THEN** the log records the focus change and no key codes
