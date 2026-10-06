import { describe, expect, it } from "vitest";
import type { JoinFailedReason, PairingClosedReason, Side } from "./bridge";
import { countdown, joinFailed, pairingClosed, side } from "./text";

describe("text", () => {
  it("has a distinct sentence for each join failure", () => {
    const reasons: JoinFailedReason[] = [
      "wrongCode",
      "notReady",
      "protocolError",
      "deskFull",
      "connectionLost",
      "unreachable",
      "mistypedCode",
      "malformedCode",
    ];
    const sentences = reasons.map(joinFailed);
    expect(new Set(sentences).size).toBe(reasons.length);
    expect(joinFailed("mistypedCode")).toMatch(/typo/);
    expect(joinFailed("notReady")).toMatch(/Show a code/);
    expect(joinFailed("invalidCode")).toBe(joinFailed("malformedCode"));
    expect(joinFailed("notConnected")).toMatch(/isn't running/);
  });

  it("has a sentence for each way pairing closes", () => {
    const reasons: PairingClosedReason[] = [
      "joined",
      "expired",
      "byUser",
      "tooManyFailures",
    ];
    expect(new Set(reasons.map(pairingClosed)).size).toBe(reasons.length);
    expect(pairingClosed("tooManyFailures")).toMatch(/wrong codes/);
  });

  it("names every side", () => {
    const sides: Side[] = ["left", "right", "top", "bottom"];
    expect(sides.map(side)).toEqual([
      "to the left of this PC",
      "to the right of this PC",
      "above this PC",
      "below this PC",
    ]);
  });

  it("formats countdowns as m:ss, rounding up", () => {
    expect(countdown(582_000)).toBe("9:42");
    expect(countdown(4_001)).toBe("0:05");
    expect(countdown(600_000)).toBe("10:00");
    expect(countdown(0)).toBe("0:00");
    expect(countdown(-50)).toBe("0:00");
  });
});
