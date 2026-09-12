/**
 * The host bridge's vocabulary — the whole surface a mounting host sees.
 *
 * A host is whatever embeds this UI: another app's panel, a browser shell, a
 * kiosk. It drives the workspace through three things and nothing else — it
 * names somewhere to go, it hears where the user went, and it hands over a
 * palette and a type stack. It never reaches into state, and the app never
 * reaches back out; a host that attaches nothing gets the desktop app.
 */

import type { WorkspaceView } from "../TopBar";
import type { SpecialPage } from "../page/types";
import type { ScryModel } from "../viewmodel";
import type { PaletteValues, ThemeConfig } from "../theme";

/** The destinations the top bar offers, in a host's words. `map` is the app's
 *  `diagram` view; `inbox` is a wiki page the top bar promotes to a peer. */
export type NavView = "wiki" | "map" | "inbox";

/** Where a host wants the UI to be. Ids are the model's own — the same strings
 *  the model file, the MCP surface and the URL bar all use. */
export type NavTarget =
  | { kind: "node"; id: string }
  | { kind: "group"; id: string }
  /** A responsibility. Resolves to whichever node or group hosts it. */
  | { kind: "claim"; id: string }
  /** An open change. Opens the Changes page pinned to that change's section. */
  | { kind: "change"; id: string }
  | { kind: "view"; id: NavView };

/** What `navigateTo` answers. A target naming something the model doesn't hold
 *  is reported, never guessed at and never silently dropped. */
export type NavResult = { ok: true } | { ok: false; reason: string };

/** What the app does about a resolved target. The pure half of navigation:
 *  `resolveNav` produces one of these, and the wiring runs it through the very
 *  callbacks the tree, the search palette and the inbox already click. */
export type NavAction =
  | { kind: "node"; id: string; flash?: string }
  | { kind: "group"; id: string; flash?: string }
  | { kind: "special"; id: SpecialPage; flash?: string }
  | { kind: "view"; id: WorkspaceView }
  | { kind: "rejected"; reason: string };

/** Where the user is. Emitted on every change, so a host's companion panel can
 *  follow the workspace without polling it. `kind: "none"` is a cleared
 *  selection (the empty diagram pane), not an error. */
export interface HostSelection {
  kind: "node" | "group" | "special" | "none";
  /** The node, group or special-page id — null when nothing is selected. */
  id: string | null;
  view: "wiki" | "map";
}

/** An 11-shade ramp, keyed exactly as the built-in palettes are. */
export type HostRamp = Record<string, string>;

/** The palette and type stack a host imposes in place of the default theme.
 *  Everything is optional: what a host leaves out keeps the app's default. */
export interface HostTheme {
  /** Role → a built-in palette name (`"azure"`), or the host's own ramp. Keys
   *  are either the theme's role tokens (`zinc`, `blue`, `red`, …) or the
   *  semantic names the theme editor shows (`neutral`, `accent`, `danger`, …). */
  palette?: Record<string, string | HostRamp>;
  /** The type stack. A bare string sets the body face; the object form also
   *  sets the monospace face used for ids, paths and code. */
  fontStack?: string | { sans?: string; mono?: string };
  /** Light, dark, or follow the system. */
  mode?: "light" | "dark" | "system";
}

/** What a host's theme spec came to. Returned so a host can see exactly what
 *  landed — including the keys that named no role, which are reported rather
 *  than guessed at. */
export interface ResolvedHostTheme {
  /** The theme config `applyTheme` takes: the app's defaults with the host's
   *  roles swapped in. */
  theme: ThemeConfig;
  /** Ramps the host supplied inline, under the names `theme` refers to them
   *  by — registered alongside the built-in palettes before the theme lands. */
  ramps: Record<string, PaletteValues>;
  /** The `--font-*` custom properties the type stack sets. */
  fonts: Record<string, string>;
  /** Palette entries that named no role and no token, or whose ramp was not a
   *  complete set of shades. Nothing was applied for these. */
  ignored: string[];
}

/** The workspace's side of the bridge: the callbacks a host's navigation is
 *  performed through. Filled in by the mounted workspace, absent before a
 *  project is open. */
export interface NavDriver {
  selectNode: (id: string) => void;
  selectGroup: (id: string) => void;
  openSpecial: (page: SpecialPage) => void;
  showView: (view: WorkspaceView) => void;
  /** Scroll an element into view and flash it, once the page has rendered. */
  flash: (elementId: string) => void;
  /** The planned model — what ids resolve against. */
  model: ScryModel;
}

/** The object a host holds. Stable for the app's whole lifetime: a host can
 *  keep the reference, subscribe once, and call it whenever. */
export interface HostBridge {
  /** Go somewhere, exactly as if the user had clicked their way there. */
  navigateTo: (target: NavTarget) => NavResult;
  /** Follow the selection. The current selection is delivered immediately, so
   *  a panel mounting late doesn't sit blank until the next click. Returns the
   *  unsubscribe. */
  onSelectionChange: (listener: (selection: HostSelection) => void) => () => void;
  /** Where the user is right now, or null before the workspace has rendered. */
  readonly selection: HostSelection | null;
  /** Impose the host's palette and type stack. Persists nothing — the host
   *  owns the preference. Answers with what actually landed. */
  applyHostTheme: (theme: HostTheme) => ResolvedHostTheme;
  /** Hand the theme back to the app's own stored preference. */
  clearHostTheme: () => void;
}

/** How a mounting host receives the bridge: a callback, or a React ref object.
 *  Passed to `App` as the `hostBridge` prop; called with the bridge on mount
 *  and with null on unmount, exactly like a ref. */
export type HostBridgeRef =
  | ((bridge: HostBridge | null) => void)
  | { current: HostBridge | null };
