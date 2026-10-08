# Spec Delta

## ADDED Requirements

### Requirement: Elevated windows
While the agent runs with UI access, input SHALL work with a window that runs elevated on the same desktop as with any other window. Injected keys, buttons, wheel and motion SHALL reach it. Physical input aimed at it SHALL be reported, and passed or withheld by the capture mode.

#### Scenario: Typing into an elevated window
- **WHEN** an elevated Notepad has focus and the agent injects a press and release of usage 0x04
- **THEN** Notepad receives the character "a"

#### Scenario: Moving over an elevated window
- **WHEN** an elevated window has focus and the agent injects a relative movement of 50 to the left
- **THEN** the cursor moves 50 counts' worth to the left

#### Scenario: Physical key while an elevated window has focus
- **WHEN** an elevated window has focus, the capture mode is `withhold all`, and the user presses a key on this machine's keyboard
- **THEN** the key is reported as physical and does not reach the elevated window

#### Scenario: Agent without UI access
- **WHEN** the agent runs without UI access and an elevated window has focus
- **THEN** injected input may not reach that window, and the agent keeps running and carrying out later commands
