// The desk's other machines, by name, with Remove behind a confirmation.
import { useState } from "react";
import type { Member } from "./bridge";
import { useBridge } from "./useMouseFlock";

export function Members({
  members,
  connected,
}: {
  members: Member[];
  connected: boolean;
}) {
  if (members.length === 0) {
    return <p className="empty">No other PCs yet.</p>;
  }
  return (
    <ul className="members">
      {members.map((m) => (
        <Row key={m.key} member={m} connected={connected} />
      ))}
    </ul>
  );
}

function Row({ member, connected }: { member: Member; connected: boolean }) {
  const bridge = useBridge();
  const [confirm, setConfirm] = useState(false);
  return (
    <li>
      <span className={member.up ? "dot up" : "dot down"} aria-hidden />
      <span className="name">{member.name}</span>
      {member.short && <span className="short">{member.short}</span>}
      <span className="state">{member.up ? "Connected" : "Not connected"}</span>
      {confirm ? (
        <span className="confirm">
          <button
            onClick={() => {
              bridge.removePeer(member.key).catch(() => undefined);
              setConfirm(false);
            }}
          >
            Yes, remove
          </button>
          <button onClick={() => setConfirm(false)}>Cancel</button>
        </span>
      ) : (
        <button
          aria-label={`Remove ${member.name}`}
          disabled={!connected}
          onClick={() => setConfirm(true)}
        >
          Remove
        </button>
      )}
    </li>
  );
}
