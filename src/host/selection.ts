/**
 * What the workspace's selection looks like from outside.
 *
 * The app's own `Selected` is internal: it carries a `SpecialPage` union and
 * says nothing about which view is showing. A host gets the flattened form —
 * a kind, an id, and the view — because that is all a companion panel needs to
 * follow along, and the less of the app's shape leaks out the less a host
 * breaks on.
 */

import type { WorkspaceView } from "../TopBar";
import type { Selected } from "../page/types";
import type { HostSelection } from "./types";

/** Flatten the workspace's selection and view into the pair a host hears. */
export function hostSelection(selected: Selected | null, view: WorkspaceView): HostSelection {
  return {
    kind: selected?.kind ?? "none",
    id: selected?.id ?? null,
    view: view === "diagram" ? "map" : "wiki",
  };
}

/** Whether two selections say the same thing. The bridge emits on CHANGE, so
 *  a re-render that lands on the same place must not wake a host up. */
export function sameSelection(a: HostSelection | null, b: HostSelection | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return a.kind === b.kind && a.id === b.id && a.view === b.view;
}
