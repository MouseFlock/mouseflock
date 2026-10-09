# Known issues

## Windows opened with "Run as administrator" don't take input from another PC

**What happens:** you control a PC from another PC, and the window in front on the controlled PC was opened with "Run as administrator", for example an admin terminal, a Notepad started from one, or Task Manager. DeskHop's input stops reaching that PC. The cursor there stops moving, so it can't reach the edge back. The keyboard and mouse you are holding seem frozen, because they still belong to the controlled PC.

**Why:** Windows doesn't let a normal program send input to a window running as administrator. DeskHop's agent runs as the logged-on user, and Windows drops its input without reporting an error.

**How to get out:** press **Scroll Lock twice** quickly on the keyboard you are using. Your keyboard and mouse come back to the PC you are sitting at, wherever the cursor was. If that keyboard has no Scroll Lock key, use the controlled PC's own mouse instead: move it twice, away from the admin window, or press a key on its own keyboard, and that PC takes control back.

**How to avoid it:** on versions before the fix below, don't leave an admin window in front on a PC you are about to control. UAC prompts, the lock screen and the logon screen are not affected; DeskHop handles those.

**Status:** fixed. The session agent now runs with UI access (ADR 0008), so input reaches elevated windows. Confirmed on two PCs on 2026-10-09: an admin Notepad and Task Manager take input, and the shield keeps the user's other programs from tampering with the agent. The recovery above applies only to versions before this fix.

## A PC whose screen you can't see

**What happens:** when two PCs share one monitor through a hardware KVM switch, the cursor can cross onto the PC the switch isn't showing. Your keyboard and mouse then drive a screen you can't see, and seem frozen.

**How to get out:** press **Scroll Lock twice** quickly to bring your keyboard and mouse back to the PC you are sitting at. Failing that, move the mouse back toward the edge you crossed, away from the corners, or press the switch's button, which moves the keyboard and mouse to that PC so it takes control back.

**Status:** open.
