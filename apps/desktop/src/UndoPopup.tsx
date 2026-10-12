// The popup near the tray after a side is learned. The Rust side shows it
// without focus and hides it on Undo or when the time runs out.
import { useState } from "react";
import type { Notice } from "./bridge";
import { side } from "./text";
import { useBridge, useNow, useSubscription } from "./useMouseFlock";

type Learned = Extract<Notice, { kind: "learned" }>;

export function UndoPopup() {
  const bridge = useBridge();
  const now = useNow();
  const [shown, setShown] = useState<{ notice: Learned; at: number } | null>(null);
  useSubscription(bridge.onUndo, (n) => {
    if (n.kind === "learned") setShown({ notice: n, at: Date.now() });
  });

  if (!shown) return null;
  const left = shown.notice.leftMs - (now - shown.at);
  if (left <= 0) return null;
  return (
    <div className="popup">
      <p>
        <strong>{shown.notice.name}</strong> is {side(shown.notice.side)}.
      </p>
      <button
        className="primary"
        onClick={() => {
          bridge.undo().catch(() => undefined);
          setShown(null);
        }}
      >
        Undo ({Math.ceil(left / 1000)})
      </button>
    </div>
  );
}
