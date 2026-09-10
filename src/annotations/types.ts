/**
 * The annotation slot's vocabulary.
 *
 * A host — anything mounting this UI — supplies MARKS per node id, and the
 * three surfaces that show nodes render them. The slot knows nothing about who
 * or why a mark exists: it is not "an avatar", it is a badge with an image; not
 * "a session is editing here", a highlight with a tone. Meaning lives with
 * whoever produced the mark.
 *
 * The desktop app supplies none, so it renders none.
 */

/** How strongly a mark reads. Purely visual; the slot never interprets it. */
export type Tone = "neutral" | "info" | "active" | "warn";

export type Mark =
  /** A small chip beside the node: a label, an image, or both. */
  | { kind: "badge"; label?: string; image?: string; tone?: Tone; title?: string }
  /** A wash over the node itself, for "something is happening here". */
  | { kind: "highlight"; label?: string; image?: string; tone?: Tone; title?: string };

/** Every mark the host is supplying right now, keyed by node id. */
export type Marks = Record<string, Mark[]>;
