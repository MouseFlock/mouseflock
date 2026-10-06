import { act, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Notice } from "./bridge";
import { UndoPopup } from "./UndoPopup";
import { connected, fakeBridge } from "./test/fakeBridge";
import { mount } from "./test/render";

const learned = (name: string, leftMs = 10_000): Notice => ({
  kind: "learned",
  key: "02".repeat(32),
  name,
  side: "right",
  leftMs,
});

async function tick(ms: number) {
  await act(async () => {
    vi.advanceTimersByTime(ms);
  });
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("UndoPopup", () => {
  it("names the machine and side, counts down, and Undo calls undo", async () => {
    const f = fakeBridge(connected(), "undo");
    const { container } = await mount(f.bridge, <UndoPopup />);
    expect(container.textContent).toBe("");

    f.popup(learned("OFFICE"));
    expect(container.textContent).toContain("OFFICE is to the right of this PC.");
    screen.getByRole("button", { name: "Undo (10)" });
    await tick(4_000);
    fireEvent.click(screen.getByRole("button", { name: "Undo (6)" }));
    expect(f.bridge.undo).toHaveBeenCalledTimes(1);
    expect(container.textContent).toBe("");
  });

  it("goes away when the time runs out, sending nothing", async () => {
    const f = fakeBridge(connected(), "undo");
    const { container } = await mount(f.bridge, <UndoPopup />);
    f.popup(learned("OFFICE"));
    await tick(10_000);
    expect(container.textContent).toBe("");
    expect(f.bridge.undo).not.toHaveBeenCalled();
  });

  it("is replaced by a newer notice", async () => {
    const f = fakeBridge(connected(), "undo");
    const { container } = await mount(f.bridge, <UndoPopup />);
    f.popup(learned("OFFICE"));
    await tick(8_000);
    f.popup(learned("LAPTOP"));
    await tick(4_000);
    expect(container.textContent).toContain("LAPTOP");
    screen.getByRole("button", { name: "Undo (6)" });
  });
});
