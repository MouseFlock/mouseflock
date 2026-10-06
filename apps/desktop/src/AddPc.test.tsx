import { act, fireEvent, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MainWindow } from "./MainWindow";
import { NO_NETWORK, joinFailed, pairingClosed } from "./text";
import { connected, fakeBridge } from "./test/fakeBridge";
import { mount } from "./test/render";

async function openAddPc(f: ReturnType<typeof fakeBridge>, choice: string) {
  await mount(f.bridge, <MainWindow />);
  fireEvent.click(screen.getByRole("button", { name: "Add a PC" }));
  fireEvent.click(screen.getByRole("button", { name: choice }));
}

const open = (leftMs: number) =>
  connected({ pairing: { state: "open", code: "137-4829153", leftMs } });

afterEach(() => {
  vi.useRealTimers();
});

describe("Show a code", () => {
  it("shows the code counting down, and Cancel closes pairing", async () => {
    vi.useFakeTimers();
    const f = fakeBridge(connected());
    await openAddPc(f, "Show a code");
    expect(f.bridge.openPairing).toHaveBeenCalledTimes(1);
    screen.getByText("Opening…");

    f.view(open(600_000));
    screen.getByText("137-4829153");
    screen.getByText("10:00 left");
    await act(async () => {
      vi.advanceTimersByTime(18_000);
    });
    screen.getByText("9:42 left");

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(f.bridge.closePairing).toHaveBeenCalledTimes(1);
    screen.getByRole("button", { name: "Show a code" });
  });

  it("does not open a second code when one is showing", async () => {
    const f = fakeBridge(open(300_000));
    await openAddPc(f, "Show a code");
    expect(f.bridge.openPairing).not.toHaveBeenCalled();
    screen.getByText("137-4829153");
  });

  it("offers a new code after too many wrong codes", async () => {
    const f = fakeBridge(connected());
    await openAddPc(f, "Show a code");
    f.view(open(500_000));
    f.notice({ kind: "pairingClosed", reason: "tooManyFailures" });
    f.view(connected());
    screen.getByText(pairingClosed("tooManyFailures"));
    fireEvent.click(screen.getByRole("button", { name: "Show a new code" }));
    expect(f.bridge.openPairing).toHaveBeenCalledTimes(2);
  });

  it("says when there is no network", async () => {
    const f = fakeBridge(connected());
    await openAddPc(f, "Show a code");
    f.notice({ kind: "pairingNotOpened", reason: "noNetwork" });
    screen.getByText(NO_NETWORK);
  });

  it("names the machine that joined", async () => {
    const f = fakeBridge(connected());
    await openAddPc(f, "Show a code");
    f.view(open(500_000));
    f.notice({ kind: "pairingClosed", reason: "joined" });
    f.notice({ kind: "paired", key: "02".repeat(32), name: "OFFICE" });
    screen.getByText("OFFICE joined.");
  });
});

describe("Enter a code", () => {
  function type(text: string) {
    fireEvent.change(screen.getByLabelText("Code shown on the other PC"), {
      target: { value: text },
    });
    fireEvent.click(screen.getByRole("button", { name: "Join" }));
  }

  it("shows joining, then a typo, keeping the text", async () => {
    const f = fakeBridge(connected());
    await openAddPc(f, "Enter a code");
    type("137-4829154");
    expect(f.bridge.join).toHaveBeenCalledWith("137-4829154");
    screen.getByText("Joining…");

    f.notice({ kind: "joinFailed", reason: "mistypedCode" });
    screen.getByText(joinFailed("mistypedCode"));
    expect(
      (screen.getByLabelText("Code shown on the other PC") as HTMLInputElement).value,
    ).toBe("137-4829154");
  });

  it("names the machine joined", async () => {
    const f = fakeBridge(connected());
    await openAddPc(f, "Enter a code");
    type("137-4829153");
    f.notice({ kind: "paired", key: "02".repeat(32), name: "OFFICE" });
    screen.getByText("This PC joined OFFICE.");
  });

  it("says when the other PC is not showing a code", async () => {
    const f = fakeBridge(connected());
    await openAddPc(f, "Enter a code");
    type("137-4829153");
    f.notice({ kind: "joinFailed", reason: "notReady" });
    screen.getByText(joinFailed("notReady"));
  });

  it("reports text the link cannot carry as not a valid code", async () => {
    const f = fakeBridge(connected());
    f.bridge.join.mockImplementationOnce(() => Promise.reject("invalidCode"));
    await openAddPc(f, "Enter a code");
    type("x".repeat(40));
    await act(async () => {});
    screen.getByText(joinFailed("invalidCode"));
  });
});
