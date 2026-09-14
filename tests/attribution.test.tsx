// @vitest-environment jsdom
/**
 * resp-chsmkg, resp-tmmzts, resp-h27gc7 — how the app names who did something.
 *
 * The model records an act as an opaque actor and, when a host ran its agent
 * on someone's say-so, the person it acted for. Rendering the record raw put
 * the word "agent" in front of a reader — the engine's internal word for a
 * machine writer, which says nothing about who decided anything — and the
 * proxy form read "by agent, as jesseh's proxy", which is a sentence about
 * bookkeeping rather than about who did the work.
 *
 * The rule, on every surface that names an actor: the agent reads as AI, an
 * act it made for a person reads as "AI on behalf of <person>", and any other
 * actor is a name a host asserted, so it passes through as given. Never the
 * word "agent", never the name of whatever product an agent runs inside, and
 * never the person's name alone — that last one is the reading the two-name
 * record exists to prevent.
 */

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { actorLabel, signatureLabel, staleNote, type SignOff } from "../src/ledger";
import { ChangesPage } from "../src/special/ChangesPage";
import { NodeHistory } from "../src/page/PageHeader";
import { planDiff } from "../src/planDiff";
import type { HistoryEvent } from "../src/history";
import { emptyModel, type ScryModel } from "../src/viewmodel";

const snapshot = (over: Partial<SignOff> = {}): SignOff => ({
  at: 1_700_000_000,
  entries: {},
  ...over,
});

/** A committed model and a plan that adds one claim, with the claim tagged to
 *  a change carrying `signedOff` — the shape the Changes page renders a
 *  signature from. */
const models = (signedOff?: SignOff): [ScryModel, ScryModel] => {
  const committed: ScryModel = {
    ...emptyModel(),
    nodes: [{ id: "n1", kind: "component", name: "Hub", responsibilities: [] }],
  };
  const planned: ScryModel = {
    ...committed,
    nodes: [
      {
        id: "n1",
        kind: "component",
        name: "Hub",
        responsibilities: [{ id: "resp-1", statement: "Verifies the token" }],
      },
    ],
    changes: [{ id: "chg-1", rationale: "Verify tokens", createdAt: 1_700_000_000, signedOff }],
    changeMap: { "resp:resp-1": "chg-1" },
  };
  return [committed, planned];
};

const event = (over: Partial<HistoryEvent> = {}): HistoryEvent => ({
  at: 1_700_000_000,
  by: "agent",
  driver: "build",
  kind: "impl",
  nodeId: "n1",
  rows: [{ marker: "+", text: "Verifies the token" }],
  ...over,
});

describe("resp-chsmkg — a sign-off the agent gave reads as the AI's", () => {
  it("resp-chsmkg: the agent signing for a person reads AI on behalf of that person", () => {
    expect(signatureLabel(snapshot({ by: "agent", onBehalfOf: "jesseh" }))).toBe(
      "AI on behalf of jesseh",
    );

    // Never the person alone — that would read as jesseh having approved it
    // himself, which is exactly what the two-name record denies. And never the
    // engine's own word for the writer.
    const label = signatureLabel(snapshot({ by: "agent", onBehalfOf: "jesseh" }));
    expect(label).not.toBe("jesseh");
    expect(label).not.toContain("agent");
  });

  it("resp-chsmkg: the agent with nobody named reads AI, and a named actor as given", () => {
    expect(signatureLabel(snapshot({ by: "agent" }))).toBe("AI");

    // An actor a host asserted is a name the app knows nothing about, so it
    // passes through — with the person beside it when there is one.
    expect(signatureLabel(snapshot({ by: "jesseh" }))).toBe("jesseh");
    expect(signatureLabel(snapshot({ by: "sam", onBehalfOf: "jesseh" }))).toBe(
      "sam on behalf of jesseh",
    );

    // Unchanged: an unattributed signature says only that one was given.
    expect(signatureLabel(snapshot())).toBeNull();
    expect(signatureLabel(snapshot({ onBehalfOf: "jesseh" }))).toBeNull();
    expect(actorLabel(undefined)).toBeNull();
  });
});

describe("resp-chsmkg / resp-tmmzts — the Changes page says it", () => {
  let host: HTMLDivElement;
  let root: Root;

  const render = (signedOff?: SignOff) => {
    const [committed, planned] = models(signedOff);
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
  };

  beforeEach(() => {
    host = document.createElement("div");
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    host.remove();
  });

  it("resp-chsmkg: the signature beside the badge, and its tooltip, both say it", () => {
    render(snapshot({ by: "agent", onBehalfOf: "jesseh" }));

    const text = host.textContent ?? "";
    expect(text).toContain("by AI on behalf of jesseh");
    expect(text).not.toContain("by agent");
    expect(text).not.toContain("proxy");

    // The hover text a reader reaches for when the badge is not enough.
    const titles = [...host.querySelectorAll("[title]")].map((el) => el.getAttribute("title") ?? "");
    expect(titles.some((t) => t.includes("Signed off") && t.includes("by AI on behalf of jesseh"))).toBe(
      true,
    );
  });

  it("resp-tmmzts: the re-signing note names the agent as AI", () => {
    render(snapshot({ by: "jesseh", staledBy: "agent" }));

    // A plan edit is an act like any other: the hand that moved it is the AI,
    // not "agent".
    expect(staleNote(snapshot({ staledBy: "agent" }))).toBe("AI has edited the plan since");
    const text = host.textContent ?? "";
    expect(text).toContain("Needs re-signing — AI has edited the plan since");
    expect(text).not.toContain("agent has edited");

    // A colleague who moved it is still named as themselves.
    expect(staleNote(snapshot({ staledBy: "sam" }))).toBe("sam has edited the plan since");
  });
});

describe("resp-h27gc7 — the node timeline says it", () => {
  let host: HTMLDivElement;
  let root: Root;

  const render = (ev: HistoryEvent) => {
    act(() => {
      root.render(<NodeHistory events={[ev]} projectPath={null} />);
    });
  };

  beforeEach(() => {
    host = document.createElement("div");
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    host.remove();
  });

  it("resp-h27gc7: a fold the agent made for a person reads AI on behalf of that person", () => {
    render(event({ by: "agent", onBehalfOf: "jesseh" }));

    const text = host.textContent ?? "";
    expect(text).toContain("AI on behalf of jesseh · build");
    expect(text).not.toContain("agent ·");
    // Not the person's own act: a fold the AI made on jesseh's say-so is not
    // jesseh having built it, so the driver is never his name alone.
    expect(actorLabel("agent", "jesseh")).not.toBe("jesseh");
    expect(text).not.toMatch(/(^|[^f] )jesseh · build/);
  });

  it("resp-h27gc7: the agent acting alone reads AI, and a named actor as given", () => {
    render(event());
    expect(host.textContent ?? "").toContain("AI · build");

    act(() => root.unmount());
    root = createRoot(host);
    render(event({ by: "sam" }));
    expect(host.textContent ?? "").toContain("sam · build");
  });

  it("resp-h27gc7: the event the app reads carries the person the engine records", () => {
    // The Rust type has carried `on_behalf_of` since proxy sign-offs existed;
    // while the client mirror did not, the person was dropped on the way in and
    // every act read as the agent's own.
    const parsed = JSON.parse(
      '{"at":1,"by":"agent","onBehalfOf":"jesseh","driver":"build","kind":"impl","nodeId":"n1","rows":[]}',
    ) as HistoryEvent;
    expect(parsed.onBehalfOf).toBe("jesseh");
    expect(actorLabel(parsed.by, parsed.onBehalfOf)).toBe("AI on behalf of jesseh");
  });
});
