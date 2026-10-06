import { act, render } from "@testing-library/react";
import type { ReactElement } from "react";
import type { Bridge } from "../bridge";
import { BridgeContext } from "../useDeskhop";

/** Renders `ui` against `bridge` and lets its first promises settle. */
export async function mount(bridge: Bridge, ui: ReactElement) {
  const result = render(<BridgeContext.Provider value={bridge}>{ui}</BridgeContext.Provider>);
  await act(async () => {});
  return result;
}

export function isDisabled(el: HTMLElement): boolean {
  return (el as HTMLButtonElement).disabled;
}
