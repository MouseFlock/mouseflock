import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { MainWindow } from "./MainWindow";
import { FORGOTTEN, NOT_RUNNING } from "./text";
import { connected, fakeBridge, member } from "./test/fakeBridge";
import { isDisabled, mount } from "./test/render";

const office = member("OFFICE", 2);

describe("MainWindow", () => {
  it("disables every command while not connected", async () => {
    const f = fakeBridge({ connected: false, status: null });
    await mount(f.bridge, <MainWindow />);
    screen.getByText(NOT_RUNNING);
    expect(isDisabled(screen.getByRole("button", { name: "Add a PC" }))).toBe(true);
    expect(isDisabled(screen.getByRole("button", { name: "Forget layout" }))).toBe(true);

    f.view(connected({ members: [office] }));
    expect(screen.queryByText(NOT_RUNNING)).toBeNull();
    expect(isDisabled(screen.getByRole("button", { name: "Remove OFFICE" }))).toBe(false);

    f.view({ connected: false, status: null });
    screen.getByText(NOT_RUNNING);
    expect(isDisabled(screen.getByRole("button", { name: "Add a PC" }))).toBe(true);
  });

  it("shows members with their state and short keys on clashes", async () => {
    const f = fakeBridge(
      connected({
        name: "DESK",
        members: [
          { ...member("OFFICE", 2), short: "02020202" },
          { ...member("OFFICE", 3, false), short: "03030303" },
        ],
        focus: { state: "on", key: "02".repeat(32), name: "OFFICE" },
      }),
    );
    await mount(f.bridge, <MainWindow />);
    screen.getByRole("heading", { name: "DESK" });
    screen.getByText("02020202");
    screen.getByText("03030303");
    screen.getByText("Connected");
    screen.getByText("Not connected");
    screen.getByText("Controlling OFFICE");
  });

  it("removes a machine only after confirming", async () => {
    const f = fakeBridge(connected({ members: [office] }));
    await mount(f.bridge, <MainWindow />);
    fireEvent.click(screen.getByRole("button", { name: "Remove OFFICE" }));
    expect(f.bridge.removePeer).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(f.bridge.removePeer).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Remove OFFICE" }));
    fireEvent.click(screen.getByRole("button", { name: "Yes, remove" }));
    expect(f.bridge.removePeer).toHaveBeenCalledWith(office.key);
  });

  it("forgets the layout only after confirming", async () => {
    const f = fakeBridge(connected());
    await mount(f.bridge, <MainWindow />);
    fireEvent.click(screen.getByRole("button", { name: "Forget layout" }));
    expect(f.bridge.forgetLayout).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Forget" }));
    expect(f.bridge.forgetLayout).toHaveBeenCalledTimes(1);
    screen.getByText(FORGOTTEN);
  });

  it("opens Add a PC when the shell asks", async () => {
    const f = fakeBridge(connected());
    await mount(f.bridge, <MainWindow />);
    expect(screen.queryByRole("button", { name: "Show a code" })).toBeNull();
    f.navigate("addPc");
    screen.getByRole("button", { name: "Show a code" });

    const g = fakeBridge(connected());
    g.bridge.route.mockResolvedValueOnce("addPc");
    await mount(g.bridge, <MainWindow />);
    expect(screen.getAllByRole("button", { name: "Show a code" })).toHaveLength(2);
  });
});
