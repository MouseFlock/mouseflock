// A Bridge for component tests: records calls, and lets a test push views,
// notices and navigation as the Rust side would.
import { act } from "@testing-library/react";
import { vi } from "vitest";
import type { Bridge, Member, Notice, Page, Status, View } from "../bridge";

type Listener<T> = (value: T) => void;

function channel<T>() {
  const listeners = new Set<Listener<T>>();
  return {
    subscribe: (f: Listener<T>) => {
      listeners.add(f);
      return Promise.resolve(() => {
        listeners.delete(f);
      });
    },
    emit: (value: T) => act(() => listeners.forEach((f) => f(value))),
  };
}

export function member(name: string, n: number, up = true): Member {
  return { key: String(n).padStart(2, "0").repeat(32), short: null, name, up };
}

export function status(over: Partial<Status> = {}): Status {
  return {
    key: "01".repeat(32),
    name: "DESK",
    agentLinked: true,
    members: [],
    pairing: { state: "closed" },
    focus: { state: "local" },
    ...over,
  };
}

export function connected(over: Partial<Status> = {}): View {
  return { connected: true, status: status(over) };
}

export function fakeBridge(initial: View, label = "main") {
  const views = channel<View>();
  const notices = channel<Notice>();
  const pages = channel<Page>();
  const undos = channel<Notice>();
  const bridge = {
    windowLabel: () => label,
    view: vi.fn(() => Promise.resolve(initial)),
    route: vi.fn(() => Promise.resolve(null as Page | null)),
    openPairing: vi.fn(() => Promise.resolve()),
    closePairing: vi.fn(() => Promise.resolve()),
    join: vi.fn((_code: string) => Promise.resolve()),
    removePeer: vi.fn((_key: string) => Promise.resolve()),
    undo: vi.fn(() => Promise.resolve()),
    forgetLayout: vi.fn(() => Promise.resolve()),
    onView: views.subscribe,
    onNotice: notices.subscribe,
    onNavigate: pages.subscribe,
    onUndo: undos.subscribe,
  } satisfies Bridge;
  return {
    bridge,
    view: views.emit,
    notice: notices.emit,
    navigate: pages.emit,
    popup: undos.emit,
  };
}
