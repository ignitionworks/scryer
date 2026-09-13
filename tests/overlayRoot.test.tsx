// @vitest-environment jsdom
/**
 * resp-rw7xhr — a guest's overlays stay in the guest's pane.
 *
 * Two halves, because neither alone is the claim. This one proves WHERE an
 * overlay attaches: given a bounded mount root, the real search palette lands
 * inside that root rather than on the document body, which is the mechanism
 * that keeps it off a host's chrome. The source half (in `hostBridge.test.ts`)
 * proves the box it then draws is `absolute inset-0` — measured against that
 * root — and that no `fixed` full-surface overlay is left anywhere.
 *
 * Geometry itself is out of reach in this suite: jsdom implements the DOM, not
 * layout, and the class names are Tailwind's with no stylesheet behind them, so
 * no `getBoundingClientRect` here would mean anything. What a browser measures
 * `inset-0` against is the positioned ancestor the overlay attaches under, and
 * that ancestor is what these assert.
 */

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { OverlayRootProvider } from "../src/host";
import { SearchPalette } from "../src/SearchPalette";
import { emptyModel } from "../src/viewmodel";

const noop = () => {};

// jsdom has no layout, so it has no `scrollIntoView` either; the palette keeps
// its active row in view on mount and would throw on the call. Nothing here
// asserts on scrolling.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = noop;
}

function palette() {
  return (
    <SearchPalette
      model={emptyModel()}
      onSelectNode={noop}
      onSelectGroup={noop}
      onClose={noop}
    />
  );
}

/** The palette's own outermost box, wherever it ended up in the document. */
function overlayBox(): HTMLElement {
  const found = document.querySelector(".z-\\[1000\\]");
  if (!found) throw new Error("the palette rendered no overlay");
  return found as HTMLElement;
}

describe("resp-rw7xhr — overlays stay in the pane they were given", () => {
  let host: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    host = document.createElement("div");
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    host.remove();
  });

  it("resp-rw7xhr: an overlay mounted in a bounded root lands inside it, not on the body", () => {
    act(() => {
      root.render(<OverlayRootProvider>{palette()}</OverlayRootProvider>);
    });

    const mount = host.firstElementChild as HTMLElement;
    const overlay = overlayBox();

    // The mount root is the containing block: positioned, and sized by whatever
    // the app was given rather than by the window.
    expect(mount.className).toContain("relative");
    expect(mount.className).toContain("h-full w-full");
    expect(mount.className).not.toMatch(/\b[hw]-screen\b/);

    // And the overlay is under it — so `inset-0` is the pane's box, and a host's
    // chrome outside the pane is not covered.
    expect(mount.contains(overlay)).toBe(true);
    expect(overlay.parentElement).not.toBe(document.body);
    expect(overlay.className).toContain("absolute inset-0");
    expect(overlay.className).not.toContain("fixed");
  });

  it("resp-rw7xhr: with no mount root above it, an overlay falls back to the body", () => {
    // A host embedding one widget on its own, or a test rendering one in
    // isolation: `absolute inset-0` under an unpositioned body resolves against
    // the initial containing block — the viewport — which is exactly what
    // `fixed` did before the mount root existed.
    act(() => {
      root.render(palette());
    });

    expect(overlayBox().parentElement).toBe(document.body);
  });
});
