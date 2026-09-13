// @vitest-environment jsdom
/**
 * resp-shsme2 — what a tree row's marks actually draw.
 *
 * The slot's logic is covered in `annotations.test.ts`; this is the render,
 * because the defect it closes was invisible to a logic test. A highlight
 * carrying a label and no image produced a correctly-toned pill with nothing
 * inside it: colour alone, which says "something is true of this row" without
 * ever saying what, and says nothing at all to a reader who cannot separate
 * the tones.
 */

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AnnotationsProvider } from "../src/annotations/context";
import { TreeRowMarks } from "../src/annotations/Annotations";
import type { Marks } from "../src/annotations/types";

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

function render(marks: Marks) {
  act(() => {
    root.render(
      <AnnotationsProvider marks={marks}>
        <TreeRowMarks nodeId="node-1" />
      </AnnotationsProvider>,
    );
  });
  return host;
}

describe("resp-shsme2 — a highlight says what it means, never colour alone", () => {
  it("resp-shsme2: a label-only highlight renders its word", () => {
    const el = render({ "node-1": [{ kind: "highlight", label: "editing", tone: "active" }] });
    expect(el.textContent).toContain("editing");
    // The tone is still carried — the word is added to the colour, not
    // instead of it.
    expect(el.innerHTML).toMatch(/class="[^"]*rounded-full/);
  });

  it("resp-shsme2: the wash is never empty", () => {
    // The defect, stated as the property that failed: a highlight is drawn, so
    // something is being said about this row — and whatever is drawn has to
    // contain the saying.
    for (const tone of ["active", "info", "warn", undefined] as const) {
      const el = render({ "node-1": [{ kind: "highlight", label: "reviewing", tone }] });
      expect(el.textContent?.trim(), `tone=${tone}`).not.toBe("");
      expect(el.textContent, `tone=${tone}`).toContain("reviewing");
    }
  });

  it("resp-shsme2: an image-only highlight stays a dot, and both draws both", () => {
    // An avatar reads as itself; there is no word to spell, and a label is not
    // invented for it.
    const img = render({
      "node-1": [{ kind: "highlight", image: "https://example.test/ada.png", tone: "active" }],
    });
    expect(img.querySelector("img")).not.toBeNull();
    expect(img.textContent?.trim()).toBe("");

    const both = render({
      "node-1": [
        { kind: "highlight", label: "editing", image: "https://example.test/ada.png", tone: "active" },
      ],
    });
    expect(both.querySelector("img")).not.toBeNull();
    expect(both.textContent).toContain("editing");
  });

  it("resp-shsme2: badges beside it still speak for themselves", () => {
    // With badges present the highlight is the wash BEHIND them and they carry
    // the words — the row is one line high and does not repeat itself.
    const el = render({
      "node-1": [
        { kind: "badge", label: "Ada" },
        { kind: "highlight", label: "editing", tone: "active" },
      ],
    });
    expect(el.textContent).toContain("Ada");
    expect(el.textContent).not.toContain("editing");
  });

  it("resp-shsme2: no marks, no element — the desktop is unchanged", () => {
    expect(render({}).innerHTML).toBe("");
  });
});
