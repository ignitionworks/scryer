/**
 * The workspace's end of the bridge: it lends the bridge its own navigation
 * callbacks, and reports where the user is.
 *
 * This is the only place the app knows a host might exist, and it knows almost
 * nothing: with no bridge every line here short-circuits, which is what makes
 * the desktop app's behaviour unchanged rather than merely similar.
 */

import { useEffect, useMemo, useRef } from "react";
import { jumpTo } from "../kit/WikiLink";
import type { WorkspaceView } from "../TopBar";
import type { Selected, SpecialPage } from "../page/types";
import type { ScryModel } from "../viewmodel";
import type { WiredHostBridge } from "./bridge";
import { NAV_FLASH_DELAY_MS } from "./navigation";
import { hostSelection } from "./selection";
import type { HostBridge } from "./types";

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
