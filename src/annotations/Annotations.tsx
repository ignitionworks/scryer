/**
 * What the three node surfaces render for a host's marks.
 *
 * One component per surface, each returning `null` when nothing is supplied —
 * so the render hooks in `ModelTree`, `NodePage` and `DiagramView` are a single
 * self-erasing element each, and everything about annotations lives here.
 */

import { useMarks } from "./context";
import { badgeClass, badgesOf, highlightClass, highlightOf, markTitle } from "./marks";
import { PILL_BASE } from "../statusColors";
import type { Mark } from "./types";

/** One badge: its image, its label, or both. */
function Badge({ mark, compact }: { mark: Mark; compact?: boolean }) {
  const title = markTitle(mark);
  const label = mark.label;
  // An image-only badge is a dot, not a pill — an avatar reads as itself.
  if (mark.image && !label) {
    return (
      <img
        src={mark.image}
        alt={title ?? ""}
        title={title}
        className={`inline-block shrink-0 rounded-full object-cover ring-1 ring-[var(--border)] ${
          compact ? "h-3.5 w-3.5" : "h-5 w-5"
        }`}
      />
    );
  }
  return (
    <span className={`${PILL_BASE} shrink-0 ${badgeClass(mark.tone)}`} title={title}>
      {mark.image && (
        <img src={mark.image} alt="" className="-ml-1 h-3.5 w-3.5 rounded-full object-cover" />
      )}
      {label}
    </span>
  );
}

/**
 * The marks on a MODEL TREE row: badges in a line beside the name, and a
 * highlight as a wash behind them. Compact — a tree row is one line high.
 */
export function TreeRowMarks({ nodeId }: { nodeId: string }) {
  const marks = useMarks(nodeId);
  if (marks.length === 0) return null;
  const badges = badgesOf(marks);
  const highlight = highlightOf(marks);
  return (
    <span
      className={`ml-1 inline-flex shrink-0 items-center gap-1 rounded-full ${
        highlight ? `px-1.5 ${highlightClass(highlight.tone)}` : ""
      }`}
      title={highlight ? markTitle(highlight) : undefined}
    >
      {highlight && !badges.length && highlight.image && (
        <Badge mark={{ ...highlight, kind: "badge" }} compact />
      )}
      {badges.map((m, i) => (
        <Badge key={i} mark={m} compact />
      ))}
    </span>
  );
}

/**
 * The marks on a NODE PAGE header, in the type line beside the kind and
 * technology: the same badges, with the highlight's LABEL spelled out — the
 * page has room to say what a wash on a tree row can only imply.
 */
export function NodePageMarks({ nodeId }: { nodeId: string }) {
  const marks = useMarks(nodeId);
  if (marks.length === 0) return null;
  const badges = badgesOf(marks);
  const highlight = highlightOf(marks);
  return (
    <span className="inline-flex items-center gap-1.5">
      {highlight && (
        <span
          className={`${PILL_BASE} ${highlightClass(highlight.tone)}`}
          title={markTitle(highlight)}
        >
          {highlight.image && (
            <img src={highlight.image} alt="" className="-ml-1 h-4 w-4 rounded-full object-cover" />
          )}
          {highlight.label}
        </span>
      )}
      {badges.map((m, i) => (
        <Badge key={i} mark={m} />
      ))}
    </span>
  );
}

/**
 * The marks on a DIAGRAM CARD: badges in the card's top-right corner, and the
 * highlight as a ring around the whole card. Absolutely positioned so the
 * card's own layout is untouched — the render hook is one element, not a
 * reflow.
 */
export function DiagramCardMarks({ nodeId }: { nodeId: string }) {
  const marks = useMarks(nodeId);
  if (marks.length === 0) return null;
  const badges = badgesOf(marks);
  const highlight = highlightOf(marks);
  return (
    <>
      {highlight && (
        <span
          aria-hidden
          className={`pointer-events-none absolute inset-0 rounded-[inherit] ${highlightClass(
            highlight.tone,
          )}`}
        />
      )}
      {(badges.length > 0 || highlight?.image) && (
        <span className="absolute -top-2 right-2 z-10 flex items-center gap-1">
          {badges.length === 0 && highlight?.image && (
            <Badge mark={{ ...highlight, kind: "badge" }} compact />
          )}
          {badges.map((m, i) => (
            <Badge key={i} mark={m} compact />
          ))}
        </span>
      )}
    </>
  );
}
