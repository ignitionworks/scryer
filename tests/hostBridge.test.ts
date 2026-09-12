/**
 * The host bridge — the three things a mounting host does, and the one thing
 * it must cost a desktop user.
 *
 * Every claim here is checked against its negative too, because the bridge's
 * whole promise is that an app with no host attached is the app that shipped
 * before it existed: navigation refuses, the selection goes nowhere, the theme
 * is untouched, and nothing is written down.
 *
 * The theme cases run against a hand-rolled DOM: enough of one for `theme.ts`
 * to paint into, and small enough that "nothing else was touched" is an
 * assertion rather than a hope.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createHostBridge } from "../src/host/bridge";
import { isStaleRevision, serviceInvoke, type HostInvoke } from "../src/host/commands";
import { changeElementId, claimHost, resolveNav } from "../src/host/navigation";
import { hostSelection, sameSelection } from "../src/host/selection";
import {
  applyHostTheme,
  clearHostTheme,
  isHostThemed,
  resolveHostTheme,
} from "../src/host/theme";
import type { HostSelection, NavDriver, OpenResult } from "../src/host/types";
import { respElementId } from "../src/SourceSection";
import { DEFAULT_THEME, PALETTES, saveTheme, SHADES } from "../src/theme";
import { emptyModel, type ScryModel } from "../src/viewmodel";

// --- fixtures ---------------------------------------------------------------

const MODEL: ScryModel = {
  ...emptyModel(),
  nodes: [
    { id: "node-1", kind: "system", name: "scryer" },
    { id: "node-2", kind: "container", name: "Desktop UI", parentId: "node-1" },
    {
      id: "node-5b6qhs",
      kind: "component",
      name: "Host Bridge",
      parentId: "node-2",
      responsibilities: [{ id: "resp-qzkjwm", statement: "**navigate** to it" }],
    },
  ],
  groups: [
    {
      id: "grp-1",
      name: "Surfaces",
      memberIds: ["node-5b6qhs"],
      responsibilities: [{ id: "resp-grp", statement: "**hold** the surfaces together" }],
    },
  ],
  changes: [{ id: "chg-g0szj7", rationale: "the host bridge", createdAt: 0 }],
};

/** A stand-in workspace: records which of its callbacks the bridge reached
 *  for, in order, so "as if the user had done it" is checkable. */
function recordingDriver(model: ScryModel = MODEL) {
  const calls: string[] = [];
  const driver: NavDriver = {
    model,
    selectNode: (id) => calls.push(`selectNode:${id}`),
    selectGroup: (id) => calls.push(`selectGroup:${id}`),
    openSpecial: (page) => calls.push(`openSpecial:${page}`),
    showView: (view) => calls.push(`showView:${view}`),
    flash: (el) => calls.push(`flash:${el}`),
  };
  return { driver, calls };
}

// --- a DOM small enough to assert about --------------------------------------

function installDom() {
  const props = new Map<string, string>();
  const classes = new Set<string>();
  const elements: { id: string; textContent: string }[] = [];
  const store = new Map<string, string>();
  let writes = 0;

  const documentElement = {
    style: {
      setProperty: (k: string, v: string) => void props.set(k, v),
      removeProperty: (k: string) => void props.delete(k),
      getPropertyValue: (k: string) => props.get(k) ?? "",
    },
    classList: {
      toggle: (name: string, on?: boolean) => {
        if (on) classes.add(name);
        else classes.delete(name);
      },
    },
  };
  const doc = {
    documentElement,
    head: { appendChild: (el: { id: string; textContent: string }) => void elements.push(el) },
    getElementById: (id: string) => elements.find((e) => e.id === id) ?? null,
    createElement: () => ({ id: "", textContent: "" }),
  };
  const win = {
    matchMedia: () => ({ matches: false, addEventListener: () => {}, removeEventListener: () => {} }),
  };
  const storage = {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => {
      writes++;
      store.set(k, v);
    },
    removeItem: (k: string) => {
      writes++;
      store.delete(k);
    },
  };

  const g = globalThis as Record<string, unknown>;
  g.document = doc;
  g.window = win;
  g.localStorage = storage;

  return {
    props,
    classes,
    elements,
    read: (k: string) => store.get(k) ?? null,
    get writes() {
      return writes;
    },
    resetWrites: () => {
      writes = 0;
    },
    uninstall: () => {
      delete g.document;
      delete g.window;
      delete g.localStorage;
    },
  };
}

type Dom = ReturnType<typeof installDom>;

describe("resp-9ev5dh — no host attached", () => {
  let dom: Dom;
  beforeEach(() => {
    dom = installDom();
  });
  afterEach(() => dom.uninstall());

  it("resp-9ev5dh: behaves exactly as the desktop app does", () => {
    // The desktop app builds no bridge at all. This is the next-worst case: one
    // built, never attached, never subscribed to — it must still do nothing.
    const bridge = createHostBridge();

    expect(bridge.navigateTo({ kind: "node", id: "node-5b6qhs" })).toEqual({
      ok: false,
      reason: "no workspace attached",
    });
    expect(bridge.navigateTo({ kind: "view", id: "map" }).ok).toBe(false);
    expect(bridge.selection).toBeNull();

    // A workspace reporting where the user is, with nobody listening, is not an
    // error and costs nothing.
    expect(() =>
      bridge.publish({ kind: "node", id: "node-1", view: "wiki" }),
    ).not.toThrow();

    // Nothing rendered, nothing themed, nothing remembered.
    expect(dom.props.size).toBe(0);
    expect(dom.elements).toEqual([]);
    expect(dom.classes.size).toBe(0);
    expect(dom.writes).toBe(0);
  });

  it("resp-9ev5dh: an unattached bridge still answers a host honestly", () => {
    const bridge = createHostBridge();
    const seen: HostSelection[] = [];
    const off = bridge.onSelectionChange((s) => seen.push(s));
    // No workspace has published yet, so there is nothing to replay — a host
    // subscribing early hears the first real selection, not a fabricated one.
    expect(seen).toEqual([]);
    off();
  });
});

describe("resp-qzkjwm — navigating where a host names", () => {
  it("resp-qzkjwm: navigates to a node, claim, change or view as if selected", () => {
    const { driver, calls } = recordingDriver();
    const bridge = createHostBridge();
    bridge.attach(driver);

    expect(bridge.navigateTo({ kind: "node", id: "node-5b6qhs" })).toEqual({ ok: true });
    expect(bridge.navigateTo({ kind: "group", id: "grp-1" })).toEqual({ ok: true });
    expect(bridge.navigateTo({ kind: "claim", id: "resp-qzkjwm" })).toEqual({ ok: true });
    expect(bridge.navigateTo({ kind: "change", id: "chg-g0szj7" })).toEqual({ ok: true });
    expect(bridge.navigateTo({ kind: "view", id: "map" })).toEqual({ ok: true });
    expect(bridge.navigateTo({ kind: "view", id: "wiki" })).toEqual({ ok: true });
    expect(bridge.navigateTo({ kind: "view", id: "inbox" })).toEqual({ ok: true });

    // Every one of these is the workspace's OWN callback — the same one the
    // tree row, the search palette and the inbox click. That is what expands
    // the tree to the node and reframes the map: the bridge adds no path of
    // its own, so a host lands exactly where a user would have.
    expect(calls).toEqual([
      "selectNode:node-5b6qhs",
      "selectGroup:grp-1",
      "selectNode:node-5b6qhs",
      "flash:resp-resp-qzkjwm",
      "openSpecial:changes",
      "flash:change-chg-g0szj7",
      "showView:diagram",
      "showView:wiki",
      "openSpecial:inbox",
    ]);
  });

  it("resp-qzkjwm: a claim is its host's page, plus a flash on the row", () => {
    // "As if the user had selected it" is literal: naming the claim does what
    // naming its node does, and then flashes the row.
    const node = resolveNav(MODEL, { kind: "node", id: "node-5b6qhs" });
    expect(resolveNav(MODEL, { kind: "claim", id: "resp-qzkjwm" })).toEqual({
      ...node,
      flash: respElementId("resp-qzkjwm"),
    });
    // A claim on a group resolves to the group's page the same way.
    expect(resolveNav(MODEL, { kind: "claim", id: "resp-grp" })).toEqual({
      kind: "group",
      id: "grp-1",
      flash: respElementId("resp-grp"),
    });
    expect(claimHost(MODEL, "resp-qzkjwm")).toEqual({ kind: "node", id: "node-5b6qhs" });
    expect(claimHost(MODEL, "resp-nope")).toBeNull();
  });

  it("resp-qzkjwm: a change opens the Changes page pinned to that change", () => {
    expect(resolveNav(MODEL, { kind: "change", id: "chg-g0szj7" })).toEqual({
      kind: "special",
      id: "changes",
      flash: changeElementId("chg-g0szj7"),
    });
    expect(changeElementId("chg-g0szj7")).toBe("change-chg-g0szj7");
  });

  it("resp-qzkjwm: a target the model does not hold is refused, never guessed", () => {
    const { driver, calls } = recordingDriver();
    const bridge = createHostBridge();
    bridge.attach(driver);

    for (const target of [
      { kind: "node", id: "node-gone" },
      { kind: "group", id: "grp-gone" },
      { kind: "claim", id: "resp-gone" },
      // A change that closed: pinning the page to it would pin it to nothing.
      { kind: "change", id: "chg-closed" },
      { kind: "view", id: "nowhere" },
      { kind: "nonsense", id: "x" },
    ] as const) {
      const result = bridge.navigateTo(target as never);
      expect(result.ok).toBe(false);
      expect(result.ok === false && result.reason).toBeTruthy();
    }
    // Nothing was selected, shown, or flashed on the way to refusing.
    expect(calls).toEqual([]);
  });

  it("resp-qzkjwm: a detached workspace stops answering, and a fresh one takes over", () => {
    const bridge = createHostBridge();
    const first = recordingDriver();
    const detach = bridge.attach(first.driver);
    bridge.navigateTo({ kind: "node", id: "node-1" });
    detach();
    // Between projects there is no workspace to navigate.
    expect(bridge.navigateTo({ kind: "node", id: "node-1" }).ok).toBe(false);
    const second = recordingDriver();
    bridge.attach(second.driver);
    bridge.navigateTo({ kind: "node", id: "node-2" });
    expect(first.calls).toEqual(["selectNode:node-1"]);
    expect(second.calls).toEqual(["selectNode:node-2"]);
  });
});

describe("resp-7pgwkq — opening a project", () => {
  it("resp-7pgwkq: opens the project a host names, exactly as the picker would", async () => {
    const opened: string[] = [];
    const bridge = createHostBridge();
    bridge.attachOpener({
      open: async (path) => {
        opened.push(path);
        // The shell answers with where it ended up — the recent list bumped,
        // the model loaded — which is the picker's own behaviour, reached
        // through the picker's own callback rather than reimplemented here.
        return { ok: true, status: "ready" };
      },
    });

    await expect(bridge.openProject("/work/acme")).resolves.toEqual({
      ok: true,
      status: "ready",
    });
    expect(opened).toEqual(["/work/acme"]);
  });

  it("resp-7pgwkq: says when there is no model to open, or the model is too old", async () => {
    const bridge = createHostBridge();
    let answer: OpenResult = { ok: true, status: "ready" };
    bridge.attachOpener({ open: async () => answer });

    // A directory with no `.scryer` in it: not an error, a thing to offer to
    // create — so a host can render the picker's own next step.
    answer = { ok: false, status: "needs-model" };
    expect(await bridge.openProject("/work/empty")).toEqual({
      ok: false,
      status: "needs-model",
    });

    // A model from a scryer too old to load.
    answer = { ok: false, status: "legacy", message: "pre-0.3 model" };
    const legacy = await bridge.openProject("/work/old");
    expect(legacy.ok).toBe(false);
    expect(legacy).toMatchObject({ status: "legacy", message: "pre-0.3 model" });

    // And everything else, carrying what the shell would have shown.
    answer = { ok: false, status: "error", message: "permission denied" };
    expect(await bridge.openProject("/work/locked")).toMatchObject({
      status: "error",
      message: "permission denied",
    });
  });

  it("resp-7pgwkq: an app that is not mounted says so rather than hanging", async () => {
    // The desktop app never builds a bridge at all; this is the next-worst
    // case, and a promise that never settles would be the worst answer of all.
    const bridge = createHostBridge();
    await expect(bridge.openProject("/work/acme")).resolves.toEqual({
      ok: false,
      status: "unattached",
    });

    // An app that unmounts stops answering, and a fresh one takes over.
    const detach = bridge.attachOpener({ open: async () => ({ ok: true, status: "ready" }) });
    expect(await bridge.openProject("/work/acme")).toEqual({ ok: true, status: "ready" });
    detach();
    expect((await bridge.openProject("/work/acme")).ok).toBe(false);
  });

  it("resp-7pgwkq: opening a project is not navigating within one", async () => {
    // A project is which model, not where in it — and it resolves against no
    // model at all, since none is loaded yet. Naming one as a nav target is
    // refused rather than quietly half-working.
    const { driver } = recordingDriver();
    const bridge = createHostBridge();
    bridge.attach(driver);
    const result = bridge.navigateTo({ kind: "project", id: "/work/acme" } as never);
    expect(result.ok).toBe(false);
  });
});

describe("resp-xrrngm — emitting the selection", () => {
  it("resp-xrrngm: emits when the node, group, special page or view changes", () => {
    const bridge = createHostBridge();
    const seen: HostSelection[] = [];
    bridge.onSelectionChange((s) => seen.push(s));

    bridge.publish(hostSelection({ kind: "node", id: "node-1" }, "wiki"));
    bridge.publish(hostSelection({ kind: "group", id: "grp-1" }, "wiki"));
    bridge.publish(hostSelection({ kind: "special", id: "changes" }, "wiki"));
    // The view alone moving is a change a companion panel has to hear.
    bridge.publish(hostSelection({ kind: "special", id: "changes" }, "diagram"));
    // A cleared selection is a selection, not a gap.
    bridge.publish(hostSelection(null, "diagram"));

    expect(seen).toEqual([
      { kind: "node", id: "node-1", view: "wiki" },
      { kind: "group", id: "grp-1", view: "wiki" },
      { kind: "special", id: "changes", view: "wiki" },
      { kind: "special", id: "changes", view: "map" },
      { kind: "none", id: null, view: "map" },
    ]);
    expect(bridge.selection).toEqual({ kind: "none", id: null, view: "map" });
  });

  it("resp-xrrngm: says nothing when nothing changed", () => {
    const bridge = createHostBridge();
    const listener = vi.fn();
    bridge.onSelectionChange(listener);

    // Re-renders are constant; a host must not be woken by each one.
    bridge.publish({ kind: "node", id: "node-1", view: "wiki" });
    bridge.publish({ kind: "node", id: "node-1", view: "wiki" });
    bridge.publish({ kind: "node", id: "node-1", view: "wiki" });
    expect(listener).toHaveBeenCalledTimes(1);

    expect(sameSelection(null, null)).toBe(true);
    expect(sameSelection(null, { kind: "none", id: null, view: "wiki" })).toBe(false);
    expect(
      sameSelection(
        { kind: "node", id: "node-1", view: "wiki" },
        { kind: "node", id: "node-1", view: "map" },
      ),
    ).toBe(false);
  });

  it("resp-xrrngm: a panel mounting late is told where the user already is", () => {
    const bridge = createHostBridge();
    bridge.publish({ kind: "node", id: "node-2", view: "wiki" });
    const seen: HostSelection[] = [];
    const off = bridge.onSelectionChange((s) => seen.push(s));
    expect(seen).toEqual([{ kind: "node", id: "node-2", view: "wiki" }]);
    off();
    bridge.publish({ kind: "node", id: "node-1", view: "wiki" });
    expect(seen).toHaveLength(1);
  });

  it("resp-xrrngm: one host's broken listener doesn't cost the others", () => {
    const bridge = createHostBridge();
    const ok = vi.fn();
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    bridge.onSelectionChange(() => {
      throw new Error("host panel blew up");
    });
    bridge.onSelectionChange(ok);
    expect(() => bridge.publish({ kind: "node", id: "node-1", view: "wiki" })).not.toThrow();
    expect(ok).toHaveBeenCalledTimes(1);
    error.mockRestore();
  });

  it("resp-xrrngm: reports the app's own selection shape, flattened", () => {
    expect(hostSelection(null, "wiki")).toEqual({ kind: "none", id: null, view: "wiki" });
    expect(hostSelection({ kind: "node", id: "node-1" }, "diagram")).toEqual({
      kind: "node",
      id: "node-1",
      view: "map",
    });
    expect(hostSelection({ kind: "special", id: "inbox" }, "wiki")).toEqual({
      kind: "special",
      id: "inbox",
      view: "wiki",
    });
  });
});

describe("resp-svcshp — speaking the service's shapes", () => {
  /** A transport that answers the way the engine service does, and records
   *  every call so what the app sent can be told from what the service got. */
  function service(plan = "{\"version\":\"0.3\"}") {
    const calls: { command: string; args?: Record<string, unknown> }[] = [];
    let revision = "rev-1";
    let refuseOnce = false;
    const transport: HostInvoke = async (command, args) => {
      calls.push({ command, args });
      if (command === "read_planned") return { revision, data: plan };
      if (command === "write_planned") {
        if (refuseOnce) {
          refuseOnce = false;
          revision = "rev-2";
          throw { error: { kind: "staleRevision", current: revision } };
        }
        revision = `rev-${calls.length}`;
        return { revision };
      }
      return `answer to ${command}`;
    };
    return {
      transport,
      calls,
      refuse: () => {
        refuseOnce = true;
      },
      get revision() {
        return revision;
      },
    };
  }

  it("resp-svcshp: unwraps the plan read the app expects to be a bare string", async () => {
    const s = service("{\"version\":\"0.3\",\"nodes\":[]}");
    const invoke = serviceInvoke(s.transport);
    // The app does `JSON.parse(await invoke("read_planned", …))` — it must get
    // the text, not the envelope the service wraps it in.
    await expect(invoke("read_planned", { refStr: "project:/work/acme" })).resolves.toBe(
      "{\"version\":\"0.3\",\"nodes\":[]}",
    );
    // And the write's `{revision}` answer is nothing to the app, as on the desktop.
    await expect(
      invoke("write_planned", { refStr: "project:/work/acme", data: "{}" }),
    ).resolves.toBeUndefined();
  });

  it("resp-svcshp: leaves every other command exactly as the app sent it", async () => {
    const s = service();
    const invoke = serviceInvoke(s.transport);
    await expect(invoke("read_model", { refStr: "project:/x" })).resolves.toBe(
      "answer to read_model",
    );
    await invoke("get_model_health", { cwd: "/x" });
    await invoke("cancel_agent_session");
    expect(s.calls.map((c) => c.command)).toEqual([
      "read_model",
      "get_model_health",
      "cancel_agent_session",
    ]);
    expect(s.calls[0].args).toEqual({ refStr: "project:/x" });
    expect(s.calls[2].args).toBeUndefined();
  });

  it("resp-svcshp: sends no base revision unless the host turns it on", async () => {
    const s = service();
    const invoke = serviceInvoke(s.transport);
    await invoke("read_planned", { refStr: "project:/work/acme" });
    await invoke("write_planned", { refStr: "project:/work/acme", data: "{}" });
    // Off by default: the app's save swallows failures, so a refusal would
    // silently drop the user's edit. Turning the guard on is the host's call.
    expect(s.calls.every((c) => !("baseRevision" in (c.args ?? {})))).toBe(true);
  });

  it("resp-svcshp: remembers the revision per project, and sends it when asked", async () => {
    const s = service();
    const invoke = serviceInvoke(s.transport, { optimistic: true });

    await invoke("read_planned", { refStr: "project:/work/acme" });
    const acmeRevision = s.revision;
    await invoke("read_planned", { cwd: "/work/other" });

    await invoke("write_planned", { refStr: "project:/work/acme", data: "{}" });
    const write = s.calls.at(-1)!;
    // The base is the one THIS project was last read at — a host may hold
    // several open, and one project's revision is meaningless to another.
    expect(write.args?.baseRevision).toBe(acmeRevision);
    expect(write.args?.data).toBe("{}");

    // `project:/x` and `/x` name one project, so they share one revision.
    await invoke("write_planned", { cwd: "/work/acme", data: "{}" });
    expect(s.calls.at(-1)!.args?.baseRevision).toBeTruthy();
  });

  it("resp-svcshp: a refused write re-reads, retries once, and reports the conflict", async () => {
    const s = service();
    const conflicts: string[] = [];
    const invoke = serviceInvoke(s.transport, {
      optimistic: true,
      onConflict: (p) => conflicts.push(p),
    });

    await invoke("read_planned", { refStr: "project:/work/acme" });
    s.refuse();
    await expect(
      invoke("write_planned", { refStr: "project:/work/acme", data: "{}" }),
    ).resolves.toBeUndefined();

    // read, write (refused), re-read, write again — once, not in a loop.
    expect(s.calls.map((c) => c.command)).toEqual([
      "read_planned",
      "write_planned",
      "read_planned",
      "write_planned",
    ]);
    // The retry carries what is current now, not what was stale.
    expect(s.calls[3].args?.baseRevision).toBe("rev-2");
    expect(conflicts).toEqual(["/work/acme"]);
  });

  it("resp-svcshp: a refusal that is not a conflict is the host's to see", async () => {
    const invoke = serviceInvoke(async () => {
      throw new Error("the service is down");
    }, { optimistic: true });
    await expect(invoke("write_planned", { cwd: "/x", data: "{}" })).rejects.toThrow(
      "the service is down",
    );

    // Hosts surface errors differently, so every shape of the service's own
    // refusal is recognised — and nothing else is.
    expect(isStaleRevision({ error: { kind: "staleRevision", current: "r" } })).toBe(true);
    expect(isStaleRevision({ kind: "staleRevision" })).toBe(true);
    expect(isStaleRevision('{"error":{"kind":"staleRevision"}}')).toBe(true);
    expect(isStaleRevision(new Error('… "kind":"staleRevision" …'))).toBe(true);
    expect(isStaleRevision(new Error("permission denied"))).toBe(false);
    expect(isStaleRevision(null)).toBe(false);
  });

  it("resp-svcshp: a transport already speaking the desktop's shape is left alone", async () => {
    // A host part-way through adopting the service still works: a bare string
    // read passes through, and nothing is remembered to send.
    const calls: string[] = [];
    const invoke = serviceInvoke(async (command) => {
      calls.push(command);
      return command === "read_planned" ? "{\"version\":\"0.3\"}" : undefined;
    }, { optimistic: true });
    await expect(invoke("read_planned", { cwd: "/x" })).resolves.toBe("{\"version\":\"0.3\"}");
    await expect(invoke("write_planned", { cwd: "/x", data: "{}" })).resolves.toBeUndefined();
    expect(calls).toEqual(["read_planned", "write_planned"]);
  });
});

describe("resp-m447bf — a host's palette and type stack", () => {
  let dom: Dom;
  beforeEach(() => {
    dom = installDom();
  });
  afterEach(() => {
    clearHostTheme();
    dom.uninstall();
  });

  const RAMP = Object.fromEntries(SHADES.map((s, i) => [s, `#0000${i.toString(16)}0`]));

  it("resp-m447bf: applies the palette through the theme's own custom properties", () => {
    applyHostTheme({ palette: { accent: RAMP }, fontStack: "Host Sans, sans-serif" });

    // The host's ramp lands on the accent role's Tailwind variables — which is
    // how every `bg-blue-500` in the app repaints without a component knowing.
    for (const [i, shade] of SHADES.entries()) {
      expect(dom.props.get(`--color-blue-${shade}`)).toBe(`#0000${i.toString(16)}0`);
    }
    expect(dom.props.get("--font-sans")).toBe("Host Sans, sans-serif");

    // And the derived surface/text/border tokens still come from `theme.ts`'s
    // own stylesheet, untouched by the bridge.
    const sheet = dom.elements.find((e) => e.id === "scryer-theme-vars");
    expect(sheet?.textContent).toContain("--surface-canvas");
    expect(sheet?.textContent).toContain("--text-muted");
  });

  it("resp-m447bf: leaves layout and components untouched", () => {
    applyHostTheme({
      palette: { danger: "rose", neutral: "slate" },
      fontStack: { sans: "Host Sans", mono: "Host Mono" },
      mode: "dark",
    });

    // Everything the host theme touches is a colour or a type variable, plus
    // the one stylesheet `theme.ts` has always owned, plus the dark class. No
    // element is added, no attribute set, no class but the colour mode.
    for (const key of dom.props.keys()) {
      expect(key.startsWith("--color-") || key.startsWith("--font-")).toBe(true);
    }
    expect(dom.elements.map((e) => e.id)).toEqual(["scryer-theme-vars"]);
    expect([...dom.classes]).toEqual(["dark"]);
  });

  it("resp-m447bf: takes role tokens and the theme's own semantic names alike", () => {
    // A host thinks in "danger", the theme in "red"; both name the same role.
    expect(resolveHostTheme({ palette: { danger: "rose" } }).theme.red).toBe("rose");
    expect(resolveHostTheme({ palette: { red: "rose" } }).theme.red).toBe("rose");
    expect(resolveHostTheme({ palette: { Accent: "violet" } }).theme.blue).toBe("violet");
    expect(resolveHostTheme({ palette: { neutral: "stone" } }).theme.zinc).toBe("stone");
    expect(resolveHostTheme({ palette: { added: "teal" } }).theme.emerald).toBe("teal");

    // What a host leaves out keeps the app's default — a palette is a partial
    // override, never a whole theme the host has to restate.
    const { theme } = resolveHostTheme({ palette: { danger: "rose" } });
    expect(theme.zinc).toBe(DEFAULT_THEME.zinc);
    expect(theme.blue).toBe(DEFAULT_THEME.blue);
  });

  it("resp-m447bf: reports what it could not use instead of guessing", () => {
    const { ignored, theme } = resolveHostTheme({
      palette: {
        nonsense: "rose",
        danger: "not-a-palette",
        // A half ramp would paint unreadable chrome; it is refused whole.
        neutral: { 50: "#ffffff", 500: "#888888" },
      },
    });
    expect(ignored.sort()).toEqual(["danger", "neutral", "nonsense"]);
    expect(theme.red).toBe(DEFAULT_THEME.red);
    expect(theme.zinc).toBe(DEFAULT_THEME.zinc);
  });

  it("resp-m447bf: sets the colour mode when the host asks, and follows the system otherwise", () => {
    expect(resolveHostTheme({ mode: "dark" }).theme.colorMode).toBe("dark");
    expect(resolveHostTheme({ mode: "light" }).theme.colorMode).toBe("light");
    expect(resolveHostTheme({}).theme.colorMode).toBe("system");

    applyHostTheme({ mode: "dark" });
    expect([...dom.classes]).toEqual(["dark"]);
    applyHostTheme({ mode: "light" });
    expect([...dom.classes]).toEqual([]);
  });

  it("resp-m447bf: a host ramp joins the built-ins rather than forking them", () => {
    const { ramps, theme } = resolveHostTheme({ palette: { agent: RAMP } });
    expect(Object.keys(ramps)).toEqual(["host:violet"]);
    expect(theme.violet).toBe("host:violet");
    applyHostTheme({ palette: { agent: RAMP } });
    // Registered alongside "azure" and the rest, so `applyTheme` selects it by
    // exactly the same rule — no second code path for host colours.
    expect(PALETTES["host:violet"]).toEqual(ramps["host:violet"]);
  });

  it("resp-m447bf: the type stack sets the body face, and the mono face on request", () => {
    expect(resolveHostTheme({ fontStack: "Host Sans" }).fonts).toEqual({
      "--font-sans": "Host Sans",
    });
    expect(resolveHostTheme({ fontStack: { mono: "Host Mono" } }).fonts).toEqual({
      "--font-mono": "Host Mono",
    });
    expect(resolveHostTheme({ fontStack: "  " }).fonts).toEqual({});
    expect(resolveHostTheme({}).fonts).toEqual({});

    // A later apply clears what the previous one set — a host swapping its
    // stack must not leave the old face behind.
    applyHostTheme({ fontStack: { sans: "One", mono: "Two" } });
    expect(dom.props.get("--font-mono")).toBe("Two");
    applyHostTheme({ fontStack: { sans: "Three" } });
    expect(dom.props.get("--font-sans")).toBe("Three");
    expect(dom.props.has("--font-mono")).toBe(false);
  });

  it("resp-m447bf: hands the theme back to the app's own when the host lets go", () => {
    expect(isHostThemed()).toBe(false);
    applyHostTheme({ palette: { accent: RAMP }, fontStack: "Host Sans" });
    expect(isHostThemed()).toBe(true);
    clearHostTheme();
    expect(isHostThemed()).toBe(false);
    expect(dom.props.has("--font-sans")).toBe(false);
  });
});

describe("resp-zxz2y8 — while a host theme is applied", () => {
  let dom: Dom;
  beforeEach(() => {
    dom = installDom();
  });
  afterEach(() => {
    clearHostTheme();
    dom.uninstall();
  });

  it("resp-zxz2y8: persists nothing, the host owning the preference", () => {
    // The user's own theme, stored the way the settings panel stores it.
    saveTheme({ ...DEFAULT_THEME, offsets: {}, blue: "rose", colorMode: "light" });
    const mine = dom.read("scryer:theme");
    expect(mine).toBeTruthy();
    dom.resetWrites();

    applyHostTheme({
      palette: { accent: "emerald", neutral: "stone" },
      fontStack: { sans: "Host Sans", mono: "Host Mono" },
      mode: "dark",
    });

    // The host's choices painted, and went nowhere near storage.
    expect(dom.props.get("--font-sans")).toBe("Host Sans");
    expect(dom.writes).toBe(0);
    expect(dom.read("scryer:theme")).toBe(mine);

    // Still nothing written when the host swaps its theme mid-session.
    applyHostTheme({ mode: "light" });
    expect(dom.writes).toBe(0);
    expect(dom.read("scryer:theme")).toBe(mine);

    // And letting go restores the user's stored preference rather than
    // overwriting it with the host's.
    clearHostTheme();
    expect(dom.writes).toBe(0);
    expect(dom.read("scryer:theme")).toBe(mine);
  });
});
