/**
 * The host bridge — the API a mounting host drives this UI through: a
 * navigation command, a selection event, and a theme hook.
 *
 * Everything a host needs is here, and everything about hosts lives under this
 * directory. Upstream carries the mount point (`App`'s `hostBridge` prop and
 * the wiring hook beside its selection state) and one anchor on the Changes
 * page, and nothing else.
 */

export { createHostBridge, type WiredHostBridge } from "./bridge";
export { isStaleRevision, serviceInvoke, type HostInvoke, type ServiceInvokeOptions } from "./commands";
export { assignRef, HostBridgeProvider, useHostBridge } from "./context";
export { changeElementId, claimHost, NAV_FLASH_DELAY_MS, resolveNav } from "./navigation";
export { hostSelection, sameSelection } from "./selection";
export {
  applyHostTheme,
  clearHostTheme,
  isHostThemed,
  resolveHostTheme,
} from "./theme";
export { useHostBridgeWiring, useHostOpener } from "./wiring";
export type {
  HostBridge,
  HostBridgeRef,
  HostRamp,
  HostSelection,
  HostTheme,
  NavAction,
  NavDriver,
  NavResult,
  NavTarget,
  NavView,
  OpenDriver,
  OpenResult,
  ResolvedHostTheme,
} from "./types";
