/**
 * What to draw for a node, decided without touching React.
 *
 * The rule the whole slot turns on: nothing supplied means nothing rendered.
 * Every helper here answers with an empty list or `undefined` rather than a
 * placeholder, so a desktop app that supplies no marks pays nothing and shows
 * nothing.
 */

import type { Mark, Marks, Tone } from "./types";

const NONE: readonly Mark[] = [];

/** The marks a host supplies for one node. Empty when it supplies none. */
export function marksFor(marks: Marks | undefined, nodeId: string): readonly Mark[] {
  if (!marks || !nodeId) return NONE;
  const list = marks[nodeId];
  return list && list.length > 0 ? list : NONE;
}

/** The badges among them — the chips a row or a header shows in a line. */
export function badgesOf(marks: readonly Mark[]): readonly Mark[] {
  return marks.length === 0 ? NONE : marks.filter((m) => m.kind === "badge");
}

/**
 * The one highlight a node reads as, when several are supplied.
 *
 * Two hosts (or two of one host's producers) can mark the same node at once —
 * two people's sessions in the same component. A node has one background, so
 * the loudest tone wins and the rest stay as badges. Deterministic, so the
 * wash does not flicker between equally-loud marks.
 */
export function highlightOf(marks: readonly Mark[]): Mark | undefined {
  let best: Mark | undefined;
  for (const m of marks) {
    if (m.kind !== "highlight") continue;
    if (!best || toneRank(m.tone) > toneRank(best.tone)) best = m;
  }
  return best;
}

const TONE_RANK: Record<Tone, number> = { neutral: 0, info: 1, active: 2, warn: 3 };

function toneRank(tone: Tone | undefined): number {
  return tone ? TONE_RANK[tone] : 0;
}

/**
 * A tone's badge classes — a tinted pill, the way every other state in this UI
 * reads (see `statusColors.ts`), so a host's mark sits inside the app's
 * vocabulary instead of beside it. Hues are deliberately not the plan-edit
 * amber or the drift orange: a mark is a third axis, not more of either.
 */
export function badgeClass(tone: Tone | undefined): string {
  switch (tone) {
    case "info":
      return "bg-blue-500/10 text-blue-700 ring-blue-500/25 dark:bg-blue-400/10 dark:text-blue-300 dark:ring-blue-400/25";
    case "active":
      return "bg-emerald-500/10 text-emerald-700 ring-emerald-500/25 dark:bg-emerald-400/10 dark:text-emerald-300 dark:ring-emerald-400/25";
    case "warn":
      return "bg-rose-500/10 text-rose-700 ring-rose-500/25 dark:bg-rose-400/10 dark:text-rose-300 dark:ring-rose-400/25";
    default:
      return "bg-[var(--surface-hover)] text-[var(--text-secondary)] ring-[var(--border)]";
  }
}

/**
 * A tone's HIGHLIGHT classes — a wash over the node itself rather than a chip
 * beside it. Lighter than the badge tint so a node under a highlight stays
 * readable.
 */
export function highlightClass(tone: Tone | undefined): string {
  switch (tone) {
    case "info":
      return "bg-blue-500/8 ring-1 ring-inset ring-blue-500/30 dark:bg-blue-400/10 dark:ring-blue-400/30";
    case "active":
      return "bg-emerald-500/8 ring-1 ring-inset ring-emerald-500/30 dark:bg-emerald-400/10 dark:ring-emerald-400/30";
    case "warn":
      return "bg-rose-500/8 ring-1 ring-inset ring-rose-500/30 dark:bg-rose-400/10 dark:ring-rose-400/30";
    default:
      return "bg-[var(--surface-hover)] ring-1 ring-inset ring-[var(--border)]";
  }
}

/** What a mark says when hovered: its own title, else its label. */
export function markTitle(mark: Mark): string | undefined {
  return mark.title ?? mark.label;
}
