/**
 * The workspace's end of the bridge: it lends the bridge its own navigation
 * callbacks, and reports where the user is.
 *
 * This is the only place the app knows a host might exist, and it knows almost
 * nothing: with no bridge every line here short-circuits, which is what makes
 * the desktop app's behaviour unchanged rather than merely similar.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { jumpTo } from "../kit/WikiLink";
import type { WorkspaceView } from "../TopBar";
import type { Selected, SpecialPage } from "../page/types";
import type { ScryModel } from "../viewmodel";
import type { ProjectStatus } from "../hooks/useModelStorage";
import type { WiredHostBridge } from "./bridge";
import { NAV_FLASH_DELAY_MS } from "./navigation";
import { hostSelection } from "./selection";
import type { HostBridge, OpenResult } from "./types";

export function useHostBridgeWiring({
  bridge,
  model,
  selected,
  view,
  selectNode,
  selectGroup,
  openSpecial,
  showView,
}: {
  bridge: HostBridge | null;
  model: ScryModel;
  selected: Selected | null;
  view: WorkspaceView;
  selectNode: (id: string) => void;
  selectGroup: (id: string) => void;
  openSpecial: (page: SpecialPage) => void;
  showView: (view: WorkspaceView) => void;
}): void {
  // Read through a ref so the bridge attaches ONCE per mount: the callbacks and
  // the model change constantly, and a host's navigation must always run
  // against the current ones, not the ones that existed when it attached.
  const latest = useRef({ model, selectNode, selectGroup, openSpecial, showView });
  latest.current = { model, selectNode, selectGroup, openSpecial, showView };

  useEffect(() => {
    if (!bridge) return;
    return (bridge as WiredHostBridge).attach({
      get model() {
        return latest.current.model;
      },
      selectNode: (id) => latest.current.selectNode(id),
      selectGroup: (id) => latest.current.selectGroup(id),
      openSpecial: (page) => latest.current.openSpecial(page),
      showView: (v) => latest.current.showView(v),
      // The page that holds the target has to render before there is anything
      // to scroll to — the same wait the inbox and needs-review jumps take.
      flash: (elementId) => {
        window.setTimeout(() => jumpTo(elementId), NAV_FLASH_DELAY_MS);
      },
    });
  }, [bridge]);

  const selection = useMemo(() => hostSelection(selected, view), [selected, view]);
  useEffect(() => {
    (bridge as WiredHostBridge | null)?.publish(selection);
  }, [bridge, selection]);
}

/**
 * The app shell's end of the bridge: opening a project.
 *
 * A level above {@link useHostBridgeWiring}, because opening happens BEFORE a
 * workspace exists — until a project is open the picker is what is mounted, and
 * the workspace's driver is not attached to answer anything. So this rides on
 * the shell, which lives for the app's whole run.
 *
 * The outcome is the awkward part: `openProject` settles the app's status
 * through React state, which a caller cannot read back the moment its promise
 * resolves. So each open takes a ticket, and the effect below hands the result
 * over once the status has left `loading` — deterministic, and no polling.
 */
export function useHostOpener({
  bridge,
  status,
  error,
  openProject,
}: {
  bridge: HostBridge | null;
  status: ProjectStatus;
  /** What the shell is showing for a failed open, passed on as the message. */
  error: string | null;
  openProject: (path: string) => Promise<void>;
}): void {
  const open = useRef(openProject);
  open.current = openProject;
  const waiting = useRef<((result: OpenResult) => void) | null>(null);
  const [ticket, setTicket] = useState(0);

  useEffect(() => {
    // Nobody asked, or the open is still running.
    if (ticket === 0 || status === "loading") return;
    const settle = waiting.current;
    if (!settle) return;
    waiting.current = null;
    settle(
      status === "ready"
        ? { ok: true, status: "ready" }
        : {
            ok: false,
            status: status === "idle" ? "error" : status,
            message: error ?? undefined,
          },
    );
  }, [ticket, status, error]);

  useEffect(() => {
    if (!bridge) return;
    return (bridge as WiredHostBridge).attachOpener({
      open: (path) =>
        new Promise<OpenResult>((resolve) => {
          // A second open while one is in flight: the first caller is answered
          // with where the app actually ended up rather than left hanging.
          waiting.current?.({ ok: false, status: "error", message: "superseded by another open" });
          waiting.current = resolve;
          setTicket((n) => n + 1);
          void open.current(path);
        }),
    });
  }, [bridge]);
}
