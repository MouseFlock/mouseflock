// The main window: this PC, its machines, Add a PC and Forget layout.
import { useEffect, useState } from "react";
import type { Focus, Page, View } from "./bridge";
import { AddPc } from "./AddPc";
import { Members } from "./Members";
import { FORGOTTEN, NOT_RUNNING } from "./text";
import { useBridge, useSubscription, useView } from "./useMouseFlock";

function focusLine(focus: Focus): string | null {
  switch (focus.state) {
    case "local":
      return null;
    case "on":
      return `Controlling ${focus.name}`;
    case "controlledBy":
      return `${focus.name} is controlling this PC`;
  }
}

export function MainWindow() {
  const bridge = useBridge();
  const { view, at } = useView();
  const [page, setPage] = useState<Page>("home");
  useEffect(() => {
    bridge.route().then((p) => {
      if (p) setPage(p);
    });
  }, [bridge]);
  useSubscription(bridge.onNavigate, setPage);

  const status = view.status;
  const focus = status ? focusLine(status.focus) : null;
  return (
    <main>
      <header>
        <h1>{status?.name ?? "MouseFlock"}</h1>
        {!view.connected && <p role="status">{NOT_RUNNING}</p>}
        {focus && <p className="focus">{focus}</p>}
      </header>
      {page === "addPc" ? (
        <AddPc view={view} at={at} onDone={() => setPage("home")} />
      ) : (
        <Home view={view} onAdd={() => setPage("addPc")} />
      )}
    </main>
  );
}

function Home({ view, onAdd }: { view: View; onAdd: () => void }) {
  return (
    <>
      <Members members={view.status?.members ?? []} connected={view.connected} />
      <div className="actions">
        <button className="primary" disabled={!view.connected} onClick={onAdd}>
          Add a PC
        </button>
      </div>
      <ForgetLayout connected={view.connected} />
    </>
  );
}

function ForgetLayout({ connected }: { connected: boolean }) {
  const bridge = useBridge();
  const [state, setState] = useState<"idle" | "confirm" | "done">("idle");
  if (state === "confirm") {
    return (
      <div className="confirm">
        <span>Forget where the other PCs are?</span>
        <button
          onClick={() => {
            bridge.forgetLayout().catch(() => undefined);
            setState("done");
          }}
        >
          Forget
        </button>
        <button onClick={() => setState("idle")}>Cancel</button>
      </div>
    );
  }
  return (
    <div className="secondary">
      <button disabled={!connected} onClick={() => setState("confirm")}>
        Forget layout
      </button>
      {state === "done" && <p role="status">{FORGOTTEN}</p>}
    </div>
  );
}
