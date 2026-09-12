/**
 * The bridge object itself — one stable thing a host holds for the app's
 * whole lifetime.
 *
 * It is a switchboard, not a store. The workspace attaches its own callbacks
 * (the same ones the tree and the search palette click) and pushes its
 * selection in; a host calls `navigateTo` and subscribes. Neither side holds
 * the other: the workspace can unmount and remount — a project closing and
 * opening — while the host keeps the same reference and the same subscription.
 *
 * Inert by construction. A bridge with nothing attached and no one listening
 * refuses navigation with a reason, drops selections on the floor, and touches
 * no DOM; the desktop app never builds one at all.
 */

import { resolveNav } from "./navigation";
import { applyHostTheme, clearHostTheme } from "./theme";
import { sameSelection } from "./selection";
import type { HostBridge, HostSelection, NavDriver, NavResult, NavTarget } from "./types";

/** The bridge plus the workspace-facing half a host never sees. */
export interface WiredHostBridge extends HostBridge {
  /** The workspace offers its callbacks. Returns the detach. */
  attach: (driver: NavDriver) => () => void;
  /** The workspace reports where the user is. Emitted only on a real change. */
  publish: (selection: HostSelection) => void;
}

export function createHostBridge(): WiredHostBridge {
  let driver: NavDriver | null = null;
  let selection: HostSelection | null = null;
  const listeners = new Set<(selection: HostSelection) => void>();

  const tell = (listener: (s: HostSelection) => void, s: HostSelection) => {
    // A host's listener is a host's code. One that throws is its own problem;
    // it must not take the workspace's render down with it.
    try {
      listener(s);
    } catch (e) {
      console.error("host bridge: selection listener failed", e);
    }
  };

  const bridge: WiredHostBridge = {
    navigateTo(target: NavTarget): NavResult {
      if (!driver) return { ok: false, reason: "no workspace attached" };
      const action = resolveNav(driver.model, target);
      switch (action.kind) {
        case "node":
          driver.selectNode(action.id);
          break;
        case "group":
          driver.selectGroup(action.id);
          break;
        case "special":
          driver.openSpecial(action.id);
          break;
        case "view":
          driver.showView(action.id);
          break;
        case "rejected":
          return { ok: false, reason: action.reason };
      }
      if ("flash" in action && action.flash) driver.flash(action.flash);
      return { ok: true };
    },

    onSelectionChange(listener) {
      listeners.add(listener);
      // A panel that mounts after the workspace would otherwise sit blank
      // until the user's next click.
      if (selection) tell(listener, selection);
      return () => {
        listeners.delete(listener);
      };
    },

    get selection() {
      return selection;
    },

    applyHostTheme,
    clearHostTheme,

    attach(next: NavDriver) {
      driver = next;
      return () => {
        if (driver === next) driver = null;
      };
    },

    publish(next: HostSelection) {
      if (sameSelection(selection, next)) return;
      selection = next;
      // Copied: a host unsubscribing from inside its own listener is normal.
      for (const listener of [...listeners]) tell(listener, next);
    },
  };

  return bridge;
}
