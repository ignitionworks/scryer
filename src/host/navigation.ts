/**
 * Resolving a host's target into what the app does about it.
 *
 * The rule the whole command turns on: a host's `navigateTo` must land the
 * user exactly where clicking would have. So nothing here re-implements
 * selection — it names which of the workspace's OWN callbacks to call, and the
 * wiring calls them. Expanding the tree to a node, framing the map on its
 * level, clearing its new-claim highlights: all of that is `selectNode`'s
 * existing job, and a host gets it by going through the same door.
 *
 * Everything in this file is pure, so what a target resolves to is testable
 * without a rendered app.
 */

import type { ScryModel } from "../viewmodel";
import { respElementId } from "../SourceSection";
import type { NavAction, NavTarget } from "./types";

/** The DOM id of a change's section on the Changes page — what pinning to a
 *  change scrolls to. The page carries it; nothing else knows the shape. */
export const changeElementId = (changeId: string) => `change-${changeId}`;

/** How long to wait before flashing, after asking for the page that holds the
 *  target. The page has to render first; this is the delay the inbox and the
 *  needs-review page already use for the same jump. */
export const NAV_FLASH_DELAY_MS = 250;

/** Whichever node or group holds a responsibility, or null when no element
 *  does — a host naming a claim from a stale cache, or from the committed
 *  model after the plan dropped it. */
export function claimHost(
  model: ScryModel,
  respId: string,
): { kind: "node" | "group"; id: string } | null {
  for (const n of model.nodes) {
    if (n.responsibilities?.some((r) => r.id === respId)) return { kind: "node", id: n.id };
  }
  for (const g of model.groups) {
    if (g.responsibilities?.some((r) => r.id === respId)) return { kind: "group", id: g.id };
  }
  return null;
}

/** What a host's target amounts to, against the model as it stands. */
export function resolveNav(model: ScryModel, target: NavTarget): NavAction {
  switch (target?.kind) {
    case "node": {
      const node = model.nodes.find((n) => n.id === target.id);
      return node ? { kind: "node", id: node.id } : rejected(`no node ${target.id}`);
    }
    case "group": {
      const group = model.groups.find((g) => g.id === target.id);
      return group ? { kind: "group", id: group.id } : rejected(`no group ${target.id}`);
    }
    case "claim": {
      const host = claimHost(model, target.id);
      if (!host) return rejected(`no claim ${target.id}`);
      // Select the claim's own page, then flash the row on it: the claim is a
      // place ON a page, never a page of its own.
      return { kind: host.kind, id: host.id, flash: respElementId(target.id) };
    }
    case "change": {
      const known = (model.changes ?? []).some((c) => c.id === target.id);
      // Pinning to a change the registry doesn't hold would open the page on
      // nothing; say so instead.
      if (!known) return rejected(`no change ${target.id}`);
      return { kind: "special", id: "changes", flash: changeElementId(target.id) };
    }
    case "view":
      if (target.id === "wiki") return { kind: "view", id: "wiki" };
      if (target.id === "map") return { kind: "view", id: "diagram" };
      // The inbox is a wiki page the top bar promotes to a destination, so
      // "go to the inbox" is a page selection, not a view switch.
      if (target.id === "inbox") return { kind: "special", id: "inbox" };
      return rejected(`no view ${String(target.id)}`);
    default:
      return rejected(`unknown target ${JSON.stringify(target)}`);
  }
}

function rejected(reason: string): NavAction {
  return { kind: "rejected", reason };
}
