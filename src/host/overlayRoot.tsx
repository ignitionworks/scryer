/**
 * The mount root every full-surface overlay lays itself over.
 *
 * A modal is the one part of an app that has no parent to answer to: it wants
 * the whole surface, so it portals out of the tree it was rendered in and sizes
 * itself to `position: fixed`. In a window that is right — the whole surface IS
 * the window. Mounted as a guest inside a host's pane it is a lie: `fixed` is
 * measured against the viewport no matter whose pane the app was given, so a
 * search palette opened in a 400px sidebar draws itself across the host's
 * chrome, its menus and everything else on screen.
 *
 * So the overlays portal HERE instead — one positioned element, rendered by the
 * app as its outermost box, sized by whatever the app was given — and lay out
 * `absolute inset-0` inside it. Standalone that element fills the window and
 * the result is pixel-for-pixel what `fixed` did; in a pane it is the pane.
 *
 * The fallback is `document.body`, for a component mounted without this
 * provider above it (a host embedding one widget, a test rendering one in
 * isolation). Body is not positioned, so `absolute inset-0` resolves against
 * the initial containing block — the viewport — which is exactly the behaviour
 * that shipped before this file existed.
 */

import { createContext, useContext, useState, type ReactNode } from "react";

const OverlayRoot = createContext<HTMLElement | null>(null);

/**
 * The app's outermost box, and the portal target for every overlay under it.
 *
 * `relative` is the whole point: it makes this element the containing block its
 * children measure `inset-0` against. `h-full w-full` takes the space the
 * mount point gives rather than asking for the window's — the same rule the
 * rest of the app follows (resp-4cjjcp).
 */
export function OverlayRootProvider({ children }: { children: ReactNode }) {
  // State, not a ref: the element does not exist on the first render, and the
  // overlays below need a re-render once it does or they would portal to the
  // fallback for the life of the mount.
  const [root, setRoot] = useState<HTMLElement | null>(null);
  return (
    <div ref={setRoot} className="relative h-full w-full">
      <OverlayRoot.Provider value={root}>{children}</OverlayRoot.Provider>
    </div>
  );
}

/** Where a full-surface overlay portals: the app's mount root, or the document
 *  body when it is mounted without one above it. */
export function useOverlayRoot(): HTMLElement {
  return useContext(OverlayRoot) ?? document.body;
}
