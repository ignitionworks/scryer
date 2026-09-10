/**
 * The annotation slot's render decisions — the part that has to be right
 * whether or not React is involved.
 *
 * The claim the whole slot turns on is the negative one: a host that supplies
 * nothing gets nothing rendered. The desktop app is exactly that host, so
 * every helper here is checked against "no marks" as well as marks.
 */

import { describe, expect, it } from "vitest";
import {
  badgeClass,
  badgesOf,
  highlightClass,
  highlightOf,
  markTitle,
  marksFor,
} from "../src/annotations/marks";
import type { Mark, Marks } from "../src/annotations/types";

const badge = (label: string, tone?: Mark["tone"]): Mark => ({ kind: "badge", label, tone });
const highlight = (label: string, tone?: Mark["tone"]): Mark => ({
  kind: "highlight",
  label,
  tone,
});

describe("no host, nothing rendered", () => {
  it("resolves to nothing for every shape of absent feed", () => {
    // The desktop app: no provider at all, so the context default is {}.
    expect(marksFor(undefined, "node-1")).toEqual([]);
    expect(marksFor({}, "node-1")).toEqual([]);
    // A host supplying marks for OTHER nodes leaves this one alone.
    expect(marksFor({ "node-2": [badge("Ada")] }, "node-1")).toEqual([]);
    // An empty list is the same as no key — never a stray empty chip.
    expect(marksFor({ "node-1": [] }, "node-1")).toEqual([]);
    // A surface asking about no node at all.
    expect(marksFor({ "node-1": [badge("Ada")] }, "")).toEqual([]);
  });

  it("derives nothing from nothing", () => {
    expect(badgesOf([])).toEqual([]);
    expect(highlightOf([])).toBeUndefined();
  });
});

describe("what a node's marks amount to", () => {
  const marks: Marks = {
    "node-1": [badge("Ada"), highlight("editing", "active"), badge("Grace")],
  };

  it("hands each surface the marks for its own node", () => {
    expect(marksFor(marks, "node-1")).toHaveLength(3);
  });

  it("separates the chips beside a node from the wash over it", () => {
    const mine = marksFor(marks, "node-1");
    expect(badgesOf(mine).map((m) => m.label)).toEqual(["Ada", "Grace"]);
    expect(highlightOf(mine)?.label).toBe("editing");
  });

  it("gives a node ONE highlight — the loudest tone wins", () => {
    // Two people's sessions in the same component: a node has one background.
    const both = [highlight("reading", "info"), highlight("editing", "active")];
    expect(highlightOf(both)?.label).toBe("editing");
    // Order must not decide it, or the wash would flicker.
    expect(highlightOf([...both].reverse())?.label).toBe("editing");
    // A tone the host left off is the quietest, never the loudest.
    expect(highlightOf([highlight("plain"), highlight("warned", "warn")])?.label).toBe("warned");
    // Equally loud: the first stays, so a steady feed renders steadily.
    const tie = [highlight("first", "active"), highlight("second", "active")];
    expect(highlightOf(tie)?.label).toBe("first");
  });

  it("never mistakes a badge for a highlight or the reverse", () => {
    expect(highlightOf([badge("Ada")])).toBeUndefined();
    expect(badgesOf([highlight("editing")])).toEqual([]);
  });
});

describe("a mark's presentation", () => {
  it("gives every tone its own styling, and an absent tone a quiet default", () => {
    const tones = ["info", "active", "warn"] as const;
    const classes = tones.map((t) => badgeClass(t));
    expect(new Set(classes).size).toBe(3);
    expect(classes).not.toContain(badgeClass(undefined));
    expect(new Set(tones.map((t) => highlightClass(t))).size).toBe(3);
    expect(badgeClass("neutral")).toBe(badgeClass(undefined));
  });

  it("says the mark's own title on hover, falling back to its label", () => {
    expect(markTitle({ kind: "badge", label: "Ada", title: "Ada is editing" })).toBe(
      "Ada is editing",
    );
    expect(markTitle({ kind: "badge", label: "Ada" })).toBe("Ada");
    // An image-only mark with neither has nothing to say, and says nothing.
    expect(markTitle({ kind: "badge", image: "/a.png" })).toBeUndefined();
  });
});

describe("the slot knows nothing about who or why", () => {
  it("carries a host's meaning without interpreting it", () => {
    // Whatever the host puts in a label or an image comes back untouched: the
    // slot has no notion of a person, a session, or a reason.
    const opaque: Mark = {
      kind: "badge",
      label: "⟨whatever the host means⟩",
      image: "https://example.invalid/avatar.png",
      title: "opaque to the slot",
    };
    const [out] = marksFor({ n: [opaque] }, "n");
    expect(out).toEqual(opaque);
  });
});
