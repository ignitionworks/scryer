/**
 * How the bridge gets into the app, and out to a host.
 *
 * Out: a mounting host passes `hostBridge` to `App` — a callback or a React
 * ref — and receives the bridge on mount, null on unmount, exactly like a ref
 * on any other component. In: the bridge is also a context value, so anything
 * rendered inside the app (a host's own panel, dropped into the annotation
 * slot) can reach it without the host threading it back down.
 *
 * The desktop app passes nothing, so no bridge is ever built and the context
 * stays null — there is no flag to set and nothing to opt out of.
 */

import { createContext, useContext, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { createHostBridge } from "./bridge";
import type { HostBridge, HostBridgeRef } from "./types";

const HostBridgeContext = createContext<HostBridge | null>(null);

/** The bridge, or null when no host is attached. */
export function useHostBridge(): HostBridge | null {
  return useContext(HostBridgeContext);
}

export function HostBridgeProvider({
  hostBridge,
  children,
}: {
  hostBridge?: HostBridgeRef;
  children: ReactNode;
}) {
  // Built once, at mount, and only when a host asked for one. A host holding
  // the reference keeps it across every project the user opens and closes.
  const [bridge] = useState<HostBridge | null>(() => (hostBridge ? createHostBridge() : null));
  // Latched, so a host passing an inline arrow doesn't re-hand the bridge on
  // every render.
  const target = useRef(hostBridge);
  target.current = hostBridge;
  useEffect(() => {
    const ref = target.current;
    if (!ref || !bridge) return;
    assignRef(ref, bridge);
    return () => assignRef(ref, null);
  }, [bridge]);
  return <HostBridgeContext.Provider value={bridge}>{children}</HostBridgeContext.Provider>;
}

/** Hand a value to either shape of ref. */
export function assignRef(ref: HostBridgeRef, bridge: HostBridge | null): void {
  if (typeof ref === "function") ref(bridge);
  else ref.current = bridge;
}
