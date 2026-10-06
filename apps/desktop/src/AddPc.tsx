// Add a PC: show this PC's code, or enter the one the other PC shows.
import { type FormEvent, useEffect, useRef, useState } from "react";
import type { CommandError, JoinFailedReason, PairingClosedReason, View } from "./bridge";
import { NO_NETWORK, countdown, joinFailed, joined, paired, pairingClosed } from "./text";
import { useBridge, useNotice, useNow } from "./useDeskhop";

type Props = { view: View; at: number; onDone: () => void };

export function AddPc({ view, at, onDone }: Props) {
  const [mode, setMode] = useState<"choose" | "show" | "enter">("choose");
  const back = () => setMode("choose");
  return (
    <section className="add">
      <h2>Add a PC</h2>
      {mode === "choose" && (
        <>
          <p>Install DeskHop on the other PC too. Then, on one of the two PCs, show a code, and type it on the other.</p>
          <div className="actions">
            <button className="primary" disabled={!view.connected} onClick={() => setMode("show")}>
              Show a code
            </button>
            <button disabled={!view.connected} onClick={() => setMode("enter")}>
              Enter a code
            </button>
          </div>
          <button className="link" onClick={onDone}>
            Back
          </button>
        </>
      )}
      {mode === "show" && <ShowCode view={view} at={at} onBack={back} onDone={onDone} />}
      {mode === "enter" && <EnterCode connected={view.connected} onBack={back} onDone={onDone} />}
    </section>
  );
}

type ShowOutcome =
  | { kind: "closed"; reason: PairingClosedReason }
  | { kind: "notOpened" }
  | { kind: "paired"; name: string };

function ShowCode({
  view,
  at,
  onBack,
  onDone,
}: {
  view: View;
  at: number;
  onBack: () => void;
  onDone: () => void;
}) {
  const bridge = useBridge();
  const now = useNow();
  const [outcome, setOutcome] = useState<ShowOutcome | null>(null);
  const started = useRef(false);
  const pairing = view.status?.pairing;

  const open = () => {
    setOutcome(null);
    bridge.openPairing().catch(() => undefined);
  };
  useEffect(() => {
    // Once per visit, and only if no code is showing already.
    if (started.current) return;
    started.current = true;
    if (pairing?.state !== "open") open();
  });
  useNotice((n) => {
    if (n.kind === "paired") setOutcome({ kind: "paired", name: n.name });
    else if (n.kind === "pairingNotOpened") setOutcome({ kind: "notOpened" });
    else if (n.kind === "pairingClosed" && n.reason !== "joined")
      setOutcome({ kind: "closed", reason: n.reason });
  });

  if (outcome?.kind === "paired") {
    return (
      <>
        <p role="status">{paired(outcome.name)}</p>
        <button className="primary" onClick={onDone}>
          Done
        </button>
      </>
    );
  }
  if (pairing?.state === "open") {
    return (
      <>
        <p>On the other PC, pick Enter a code and type:</p>
        <p className="code">{pairing.code}</p>
        <p className="left">{countdown(pairing.leftMs - (now - at))} left</p>
        <button
          onClick={() => {
            bridge.closePairing().catch(() => undefined);
            onBack();
          }}
        >
          Cancel
        </button>
      </>
    );
  }
  if (outcome) {
    return (
      <>
        <p role="alert">{outcome.kind === "notOpened" ? NO_NETWORK : pairingClosed(outcome.reason)}</p>
        <div className="actions">
          <button className="primary" disabled={!view.connected} onClick={open}>
            {outcome.kind === "notOpened" ? "Try again" : "Show a new code"}
          </button>
          <button onClick={onBack}>Back</button>
        </div>
      </>
    );
  }
  return <p>Opening…</p>;
}

type JoinState =
  | { kind: "idle" }
  | { kind: "joining" }
  | { kind: "failed"; reason: JoinFailedReason | CommandError }
  | { kind: "joined"; name: string };

function EnterCode({
  connected,
  onBack,
  onDone,
}: {
  connected: boolean;
  onBack: () => void;
  onDone: () => void;
}) {
  const bridge = useBridge();
  const [text, setText] = useState("");
  const [state, setState] = useState<JoinState>({ kind: "idle" });
  useNotice((n) => {
    if (state.kind !== "joining") return;
    if (n.kind === "joinFailed") setState({ kind: "failed", reason: n.reason });
    else if (n.kind === "paired") setState({ kind: "joined", name: n.name });
  });

  if (state.kind === "joined") {
    return (
      <>
        <p role="status">{joined(state.name)}</p>
        <button className="primary" onClick={onDone}>
          Done
        </button>
      </>
    );
  }
  const submit = (e: FormEvent) => {
    e.preventDefault();
    setState({ kind: "joining" });
    bridge.join(text).catch((reason: CommandError) => setState({ kind: "failed", reason }));
  };
  const joining = state.kind === "joining";
  return (
    <form onSubmit={submit}>
      <label>
        Code shown on the other PC
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="137-4829153"
          autoFocus
          spellCheck={false}
        />
      </label>
      {state.kind === "failed" && <p role="alert">{joinFailed(state.reason)}</p>}
      {joining && <p role="status">Joining…</p>}
      <div className="actions">
        <button className="primary" type="submit" disabled={!connected || joining || text.trim() === ""}>
          Join
        </button>
        <button type="button" onClick={onBack}>
          Back
        </button>
      </div>
    </form>
  );
}
