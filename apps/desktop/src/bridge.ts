// The only file that talks to Tauri. Everything the UI shows comes from the
// service through the Rust side; tests replace this with a fake.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type Member = {
  key: string;
  /** Set only when another member has the same name. */
  short: string | null;
  name: string;
  up: boolean;
};

export type Pairing =
  | { state: "closed" }
  | { state: "open"; code: string; leftMs: number };

export type Focus =
  | { state: "local" }
  | { state: "on"; key: string; name: string }
  | { state: "controlledBy"; key: string; name: string };

export type Status = {
  key: string;
  name: string;
  agentLinked: boolean;
  members: Member[];
  pairing: Pairing;
  focus: Focus;
};

export type View = { connected: boolean; status: Status | null };

export type JoinFailedReason =
  | "wrongCode"
  | "notReady"
  | "protocolError"
  | "deskFull"
  | "connectionLost"
  | "unreachable"
  | "mistypedCode"
  | "malformedCode";

export type PairingClosedReason =
  | "joined"
  | "expired"
  | "byUser"
  | "tooManyFailures";

export type Side = "left" | "right" | "top" | "bottom";

export type Notice =
  | { kind: "paired"; key: string; name: string }
  | { kind: "joinFailed"; reason: JoinFailedReason }
  | { kind: "pairingClosed"; reason: PairingClosedReason }
  | { kind: "pairingNotOpened"; reason: "noNetwork" }
  | { kind: "learned"; key: string; name: string; side: Side; leftMs: number };

/** Why a command was not sent; commands reject with one of these. */
export type CommandError = "notConnected" | "invalidCode" | "invalidKey";

export type Page = "home" | "addPc";

type Unlisten = () => void;

export interface Bridge {
  windowLabel(): string;
  view(): Promise<View>;
  /** The page the main window should open, if the shell asked for one. */
  route(): Promise<Page | null>;
  openPairing(): Promise<void>;
  closePairing(): Promise<void>;
  join(code: string): Promise<void>;
  removePeer(key: string): Promise<void>;
  undo(): Promise<void>;
  forgetLayout(): Promise<void>;
  onView(f: (view: View) => void): Promise<Unlisten>;
  onNotice(f: (notice: Notice) => void): Promise<Unlisten>;
  onNavigate(f: (page: Page) => void): Promise<Unlisten>;
  /** The Undo popup's notice. */
  onUndo(f: (notice: Notice) => void): Promise<Unlisten>;
}

function on<T>(event: string, f: (payload: T) => void): Promise<Unlisten> {
  return listen<T>(event, (e) => f(e.payload));
}

export const tauriBridge: Bridge = {
  windowLabel: () => getCurrentWindow().label,
  view: () => invoke<View>("view"),
  route: () => invoke<Page | null>("route"),
  openPairing: () => invoke("open_pairing"),
  closePairing: () => invoke("close_pairing"),
  join: (code) => invoke("join", { code }),
  removePeer: (key) => invoke("remove_peer", { key }),
  undo: () => invoke("undo"),
  forgetLayout: () => invoke("forget_layout"),
  onView: (f) => on("mouseflock://view", f),
  onNotice: (f) => on("mouseflock://notice", f),
  onNavigate: (f) => on("mouseflock://navigate", f),
  onUndo: (f) => on("mouseflock://undo", f),
};
