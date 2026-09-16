/**
 * The change ledger, canvas side — named partitions of the plan.
 *
 * Mirrors `scryer-core/src/changes.rs`: the plan carries an open-change
 * registry (`model.changes`, each with the dev's rationale) and a side-map
 * (`model.changeMap`) from element key to change id. The canvas stamps the
 * ACTIVE change onto whatever an edit touched at the `updateModel` chokepoint
 * — the mirror of the MCP server tagging each tool write — so dev-on-canvas
 * and agent-over-MCP attribute into the same ledger. Untagged elements are
 * the unfiled bucket (the zero-friction serial workflow). Keys must match the
 * Rust side byte-for-byte: they are how the two writers agree on identity.
 */

import type { ElementChange, ElementKind, ModelDiff } from "./planDiff";
import { planDiff } from "./planDiff";
import type { ScryModel } from "./viewmodel";

export interface ChangeMeta {
  /** Stable id, minted `chg-N` — same mint rule as the Rust side. */
  id: string;
  /** The dev's original sentence — why this change exists. */
  rationale: string;
  /** Unix seconds. */
  createdAt: number;
  /** The developer's sign-off, when given: a snapshot of every entry tagged
   *  to the change at that moment. Agent plan writes afterwards are classified
   *  against it (amendment / addition) and land as vagrant for a verdict.
   *  Mirrors Rust `ChangeMeta.signed_off`. */
  signedOff?: SignOff;
}

/** One sign-off snapshot: when it was stamped, who by, and each tagged entry's
 *  signed content, keyed by {@link elementKey}. Mirrors Rust
 *  `changes::SignOff`; every field but `at` and `entries` is absent on a
 *  signature that has nothing to say about it. */
export interface SignOff {
  /** Unix seconds. */
  at: number;
  /** WHO gave the go-ahead, when the signature named an actor. Absent on an
   *  unattributed one, and on every plan written before the field existed. */
  by?: string;
  /** The PERSON {@link SignOff.by} signed FOR, when they signed as that
   *  person's proxy — a host's agent approving on a developer's say-so records
   *  the agent in `by` and the developer here. Both names or neither: a reader
   *  shown only one cannot tell "X signed for Y" from "Y signed". */
  onBehalfOf?: string;
  /** WHOSE plan write moved the plan on since this signature, so the snapshot
   *  is no longer what the plan holds and the signer has not seen the
   *  difference. The name is the flag — only a write by an actor other than
   *  the signer sets it, and it never says "stale" without saying by whom. */
  staledBy?: string;
  entries: Record<string, SignedEntry>;
}

/** The actor an act records when no name was given for it — the engine's own
 *  word for a machine writer, and the serde default of Rust
 *  `history::HistoryEvent.by`. The one actor string the app can recognise:
 *  every other one is opaque, a name some host asserted. */
export const AGENT_ACTOR = "agent";

/** How an act reads to a person: the actor, and the person it was done for.
 *
 *  The agent gets a name a reader recognises — "AI" — rather than the model's
 *  internal word, and an act it made for someone reads as that person's, done
 *  by the AI: "AI on behalf of morgan". Never the bare actor, never the
 *  person's name alone (that would read as the person having done it
 *  themselves, which is the one reading the two-name record exists to
 *  prevent), and never the name of whatever product the agent runs inside —
 *  the app has no such name to show, and the engine never records one.
 *
 *  Any other actor is a name a host asserted and the app knows nothing about,
 *  so it passes through as given. `null` when nobody is named at all. */
export function actorLabel(by: string | undefined, onBehalfOf?: string): string | null {
  if (!by) return null;
  const who = by === AGENT_ACTOR ? "AI" : by;
  return onBehalfOf ? `${who} on behalf of ${onBehalfOf}` : who;
}

/** What a stale signature needs said: who moved the plan out from under it.
 *  `null` when the signature still covers what the plan holds — which is every
 *  signature in a project only one person writes to. */
export function staleNote(signedOff: SignOff | undefined): string | null {
  const who = actorLabel(signedOff?.staledBy);
  return who ? `${who} has edited the plan since` : null;
}

/** How a sign-off reads to a person: "morgan", or "AI on behalf of morgan"
 *  when the agent gave it on their say-so. `null` when nobody is named — an
 *  unattributed signature says only that one was given. */
export function signatureLabel(signedOff: SignOff | undefined): string | null {
  return actorLabel(signedOff?.by, signedOff?.onBehalfOf);
}

/** What a sign-off remembered about one entry. */
export interface SignedEntry {
  /** Hash of the entry's truth-bearing fields at sign-off. */
  hash: string;
  /** For a responsibility: the approved statement. */
  statement?: string;
  /** For a responsibility: the host it sat on at sign-off. */
  host?: string;
}

/** Canonical change-map key for an element — kind-prefixed, properties keyed
 *  by `(owner node, label)`. Must equal `changes::element_key` in Rust. */
export function elementKey(kind: ElementKind, ownerId: string | undefined, id: string): string {
  switch (kind) {
    case "node":
      return `node:${id}`;
    case "link":
      return `link:${id}`;
    case "group":
      return `group:${id}`;
    case "responsibility":
      return `resp:${id}`;
    case "property":
      return `prop:${ownerId ?? ""}:${id}`;
  }
}

/** The key of a diff entry — the join point between the map and the diff. */
export function keyFor(ec: ElementChange): string {
  return elementKey(ec.kind, ec.ownerId, ec.id);
}

/** Tag what an edit changed to the active change: the keys of
 *  `diff(prev, next)` — exactly what THIS edit touched, deletions included,
 *  the same computation the MCP write path uses. Last writer wins a re-tag
 *  (the collision itself is surfaced agent-side at write time). No-op when
 *  the edit touched nothing truth-bearing (a drag re-tags nothing) or the
 *  change is not in this plan's registry (it closed under us). */
export function tagEdit(prev: ScryModel, next: ScryModel, changeId: string): ScryModel {
  if (!(next.changes ?? []).some((c) => c.id === changeId)) return next;
  const keys = planDiff(prev, next).changes.map(keyFor);
  if (keys.length === 0) return next;
  const map = { ...(next.changeMap ?? {}) };
  for (const k of keys) map[k] = changeId;
  return { ...next, changeMap: map };
}

/** Which changes an aggregated plan entry participates in — the tags on its
 *  own element key and on every child/link element it carries. Keys whose
 *  entries the diff no longer holds are naturally ignored (the display-side
 *  analogue of the Rust gc invariant). Empty set = unfiled. */
export function entryChanges(
  entryKind: "node" | "group",
  entryId: string,
  parts: ElementChange[],
  changeMap: Record<string, string> | undefined,
): Set<string> {
  const out = new Set<string>();
  if (!changeMap) return out;
  const own = changeMap[elementKey(entryKind, undefined, entryId)];
  if (own) out.add(own);
  for (const ec of parts) {
    const tag = changeMap[keyFor(ec)];
    if (tag) out.add(tag);
  }
  return out;
}

/** Per-change count of live pending entries (diff-backed, stale tags don't
 *  count) — what the section headers and the powerline report. */
export function liveEntryCounts(
  diff: ModelDiff,
  changeMap: Record<string, string> | undefined,
): Map<string, number> {
  const counts = new Map<string, number>();
  if (!changeMap) return counts;
  for (const ec of diff.changes) {
    const tag = changeMap[keyFor(ec)];
    if (tag) counts.set(tag, (counts.get(tag) ?? 0) + 1);
  }
  return counts;
}
