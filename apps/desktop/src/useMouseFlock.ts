// React's view of the service: the latest View, notices as they happen,
// and a ticking clock for countdowns. No decisions are made here.
import { createContext, useContext, useEffect, useRef, useState } from "react";
import { type Bridge, type Notice, type View, tauriBridge } from "./bridge";

export const BridgeContext = createContext<Bridge>(tauriBridge);

export function useBridge(): Bridge {
  return useContext(BridgeContext);
}

const DISCONNECTED: View = { connected: false, status: null };

/** Subscribes `f` for the component's lifetime, latest `f` always used. */
export function useSubscription<T>(
  subscribe: (f: (value: T) => void) => Promise<() => void>,
  f: (value: T) => void,
): void {
  const latest = useRef(f);
  latest.current = f;
  useEffect(() => {
    let off: (() => void) | undefined;
    let gone = false;
    subscribe((v) => latest.current(v)).then((u) => {
      if (gone) u();
      else off = u;
    });
    return () => {
      gone = true;
      off?.();
    };
  }, [subscribe]);
}

/** The latest view, and when it arrived (for counting down `leftMs`). */
export function useView(): { view: View; at: number } {
  const bridge = useBridge();
  const [state, setState] = useState({ view: DISCONNECTED, at: Date.now() });
  useEffect(() => {
    let gone = false;
    bridge.view().then((view) => {
      if (!gone) setState({ view, at: Date.now() });
    });
    return () => {
      gone = true;
    };
  }, [bridge]);
  useSubscription(bridge.onView, (view) => setState({ view, at: Date.now() }));
  return state;
}

export function useNotice(f: (notice: Notice) => void): void {
  useSubscription(useBridge().onNotice, f);
}

/** The current time, updated every `ms` while mounted. */
export function useNow(ms = 250): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(id);
  }, [ms]);
  return now;
}
