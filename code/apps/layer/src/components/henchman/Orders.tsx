import { useState } from "react";
import { api } from "../../lib/api";
import { useOrders } from "../../lib/henchman-orders";
import { ErrorMessage, Modal, useFailure } from "../ui";

/**
 * The Henchman's standing orders and what it leaves alone (HEN-7, HEN-8):
 * what it answers for the user without asking, in the user's own words.
 * They are mostly made by telling it so; here they are read, written and
 * taken back, in a dialog from the lair's foot.
 */
export function OrdersDialog({ onClose }: { onClose: () => void }) {
  const orders = useOrders((s) => s.orders);
  const set = useOrders((s) => s.set);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const failure = useFailure();

  async function run(change: () => ReturnType<typeof api.henchmanOrders>) {
    setBusy(true);
    failure.clear();
    try {
      set(await change());
      return true;
    } catch (e) {
      failure.fail(e);
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function add() {
    if (!text.trim()) return;
    if (await run(() => api.addHenchmanOrder(text))) setText("");
  }

  return (
    <Modal title="Standing orders" onClose={onClose}>
      {orders && orders.orders.length + orders.left_alone.length === 0 && (
        <div className="lair-calm">
          None yet. Tell the Henchman what to always answer, or write it here.
        </div>
      )}
      {orders?.orders.map((o) => (
        <div key={o.id} className="order-row">
          <div className="t" title={o.text}>
            {o.text}
          </div>
          <button
            className="ui-link"
            disabled={busy}
            title="The Henchman asks you again"
            onClick={() => void run(() => api.removeHenchmanOrder(o.id))}
          >
            Remove
          </button>
        </div>
      ))}
      {orders?.left_alone.map((a) => (
        <div key={a.key} className="order-row">
          <div className="t" title={a.name}>
            {a.name}
          </div>
          <div className="why">left alone</div>
          <button
            className="ui-link"
            disabled={busy}
            title="Let the Henchman drive it again"
            onClick={() => void run(() => api.henchmanLeaveAlone(a.key, false))}
          >
            Drive it
          </button>
        </div>
      ))}
      <input
        autoFocus
        className="ui-field order-add"
        value={text}
        disabled={busy}
        placeholder="Always…, e.g. use bun, not npm"
        aria-label="A new standing order"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.nativeEvent.isComposing) void add();
        }}
      />
      <ErrorMessage>{failure.error}</ErrorMessage>
    </Modal>
  );
}
