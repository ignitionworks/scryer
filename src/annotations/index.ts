/**
 * The annotation slot — a host supplies marks per node id, and the model tree,
 * node page and diagram cards render them. The desktop app supplies none.
 *
 * Everything about annotations lives under this directory; upstream's
 * components carry one self-erasing element each and nothing else.
 */

export { AnnotationsProvider, useAllMarks, useMarks } from "./context";
export { DiagramCardMarks, NodePageMarks, TreeRowMarks } from "./Annotations";
export { badgeClass, badgesOf, highlightClass, highlightOf, markTitle, marksFor } from "./marks";
export type { Mark, Marks, Tone } from "./types";
