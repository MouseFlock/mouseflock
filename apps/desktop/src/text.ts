// Every sentence the user reads, in one place. Pure functions only.
import type {
  CommandError,
  JoinFailedReason,
  PairingClosedReason,
  Side,
} from "./bridge";

export const NOT_RUNNING =
  "MouseFlock's service isn't running on this PC. This window connects as soon as it starts.";

export function joinFailed(reason: JoinFailedReason | CommandError): string {
  switch (reason) {
    case "wrongCode":
      return "That code didn't match. Check the code on the other PC and try again.";
    case "notReady":
      return "The other PC isn't showing a code. Pick Show a code on it first.";
    case "protocolError":
      return "Something went wrong while joining. Try again.";
    case "deskFull":
      return "The other PC's group is full.";
    case "connectionLost":
      return "The connection to the other PC dropped. Try again.";
    case "unreachable":
      return "Couldn't reach the other PC. Make sure both PCs are on the same network.";
    case "mistypedCode":
      return "That code has a typo. Check it against the other PC.";
    case "malformedCode":
    case "invalidCode":
    case "invalidKey":
      return "That isn't a valid code. A code looks like 137-4829153.";
    case "notConnected":
      return NOT_RUNNING;
  }
}

export function pairingClosed(reason: PairingClosedReason): string {
  switch (reason) {
    case "joined":
      return "A PC joined with this code.";
    case "expired":
      return "The code expired.";
    case "byUser":
      return "The code was cancelled.";
    case "tooManyFailures":
      return "Too many wrong codes were tried, so this code was closed.";
  }
}

export const NO_NETWORK = "Connect this PC to a network first.";

/** Where a learned machine is, as in "OFFICE is to the right of this PC". */
export function side(s: Side): string {
  switch (s) {
    case "left":
      return "to the left of this PC";
    case "right":
      return "to the right of this PC";
    case "top":
      return "above this PC";
    case "bottom":
      return "below this PC";
  }
}

/** Time left as m:ss, rounded up to the whole second. */
export function countdown(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

export function joined(name: string): string {
  return `This PC joined ${name}.`;
}

export function paired(name: string): string {
  return `${name} joined.`;
}

export const FORGOTTEN =
  "Layout forgotten. The next push across an edge learns it again.";
