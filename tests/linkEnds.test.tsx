// @vitest-environment jsdom
/**
 * resp-wsbj76 — a link change carries the two ends it joins.
 *
 * A link's label is a verb phrase: "Spawns change sessions from" names neither
 * what spawns nor what is spawned. The Changes page could always reconstruct
 * the ends from the model, so it looked correct; every other reader of the
 * diff — `get_pending`, which is what an AGENT sees, and any surface mounting
 * this one — got the verb alone. The ends travel with the change now, and this
 * checks both the shape and the row that renders it.
 */

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ChangesPage } from "../src/special/ChangesPage";
import { planDiff } from "../src/planDiff";
import { emptyModel, type ScryModel } from "../src/viewmodel";

const LABEL = "Spawns change sessions from";

const model = (links: ScryModel["links"]): ScryModel => ({
  ...emptyModel(),
  nodes: [
    { id: "n1", kind: "component", name: "Hub" },
    { id: "n2", kind: "component", name: "Runner" },
  ],
  links,
});

const link = (over: Partial<ScryModel["links"][number]> = {}) => ({
  id: "l1",
  src: "n1",
  dst: "n2",
  label: LABEL,
  ...over,
});

describe("resp-wsbj76 — a link change names both its ends", () => {
  it("resp-wsbj76: an added link carries source and destination", () => {
    const [ec] = planDiff(model([]), model([link()])).changes;
    expect(ec.kind).toBe("link");
    expect(ec.from).toBe("n1");
    expect(ec.to).toBe("n2");
  });

  it("resp-wsbj76: a reworded link carries them too", () => {
    const before = model([link()]);
    const after = model([link({ label: "Spawns sessions for" })]);
    const [ec] = planDiff(before, after).changes;
    expect(ec.from).toBe("n1");
    expect(ec.to).toBe("n2");
  });

  it("resp-wsbj76: a DROPPED link names the ends the plan no longer has", () => {
    // The case a model lookup cannot serve: the link is gone from the planned
    // model, so the only copy of its ends is in the layer that still holds it.
    const [ec] = planDiff(model([link()]), model([])).changes;
    expect(ec.changes[0].type).toBe("deleted");
    expect(ec.from).toBe("n1");
    expect(ec.to).toBe("n2");
  });

  it("resp-wsbj76: nothing but a link carries ends", () => {
    const before = emptyModel();
    const after: ScryModel = { ...emptyModel(), nodes: [{ id: "n1", kind: "component", name: "Hub" }] };
    for (const ec of planDiff(before, after).changes) {
      expect(ec.from, ec.kind).toBeUndefined();
      expect(ec.to, ec.kind).toBeUndefined();
    }
  });
});

describe("resp-wsbj76 — the row states both ends", () => {
  let host: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    host = document.createElement("div");
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    host.remove();
  });

  it("resp-wsbj76: the Changes page row names the source rather than implying it", () => {
    const committed = model([]);
    const planned = model([link()]);
    act(() => {
      root.render(
        <ChangesPage
          planDiff={planDiff(committed, planned)}
          model={planned}
          committed={committed}
          changeLog={[]}
          onSelectNode={() => {}}
        />,
      );
    });
    // Both ends on the row itself, in order, around the verb — not one of them
    // left to the card the row happens to sit under.
    const text = host.textContent ?? "";
    expect(text).toContain(`Hub→${LABEL}→Runner`);
  });
});
