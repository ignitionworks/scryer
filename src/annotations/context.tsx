/**
 * The annotation slot itself: one React context through which a host supplies
 * marks per node id.
 *
 * The default value is EMPTY, so an app that never mounts a provider — the
 * desktop app — supplies no marks and renders none, with nothing to opt out
 * of. A host wraps the app in `<AnnotationsProvider marks={…}>` and every
 * surface picks them up.
 *
 * The slot is deliberately ignorant. It never asks who a mark is for or why it
 * exists; that meaning belongs to whoever produced it.
 */

import { createContext, useContext, useMemo } from "react";
import type { ReactNode } from "react";
import { marksFor } from "./marks";
import type { Mark, Marks } from "./types";

const EMPTY: Marks = {};

const AnnotationsContext = createContext<Marks>(EMPTY);

export function AnnotationsProvider({
  marks,
  children,
}: {
  marks: Marks | undefined;
  children: ReactNode;
}) {
  // A host that rebuilds its marks object every render (a live feed does)
  // would otherwise re-render every consumer on every tick.
  const value = useMemo(() => marks ?? EMPTY, [marks]);
  return <AnnotationsContext.Provider value={value}>{children}</AnnotationsContext.Provider>;
}

/** Every mark supplied for one node. Empty when the host supplies none. */
export function useMarks(nodeId: string | undefined): readonly Mark[] {
  const marks = useContext(AnnotationsContext);
  return marksFor(marks, nodeId ?? "");
}

/** The whole feed, for a surface that draws many nodes at once. */
export function useAllMarks(): Marks {
  return useContext(AnnotationsContext);
}
