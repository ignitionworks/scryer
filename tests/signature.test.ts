/**
 * resp-z5zzvj — what a reader of the Changes page can tell about a signature.
 *
 * The ledger records two names for a proxy sign-off and one for a direct one,
 * and the difference is the whole point: an agent a host runs on a developer's
 * say-so approving a change is not the developer approving it. A surface that
 * shows only `by` turns the first into the second silently.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { signatureLabel, staleNote, type SignOff } from "../src/ledger";

const snapshot = (over: Partial<SignOff> = {}): SignOff => ({
  at: 1_700_000_000,
  entries: {},
  ...over,
});

describe("resp-z5zzvj — a signature says who signed, and who for", () => {
  it("resp-z5zzvj: reads a proxy signature apart from the person's own", () => {
    expect(signatureLabel(snapshot({ by: "jesseh" }))).toBe("jesseh");
    expect(
      signatureLabel(snapshot({ by: "claude-session-7", onBehalfOf: "jesseh" })),
    ).toBe("claude-session-7 for jesseh");

    // Never the person alone: that is precisely the reading the record exists
    // to prevent.
    expect(signatureLabel(snapshot({ by: "claude-session-7", onBehalfOf: "jesseh" }))).not.toBe(
      "jesseh",
    );
  });

  it("resp-z5zzvj: says nothing when nobody was named, and nothing about no signature", () => {
    // An unattributed sign-off — the desktop's, and every plan written before
    // the field existed — says only that one was given. No invented name.
    expect(signatureLabel(snapshot())).toBeNull();
    expect(signatureLabel(snapshot({ onBehalfOf: "jesseh" }))).toBeNull();
    expect(signatureLabel(undefined)).toBeNull();
  });

  it("resp-h4rf4g: a stale signature says it needs re-signing, and who moved the plan", () => {
    // "Out of date" alone sends the signer looking. The hand that moved it is
    // the difference between that and a question they can ask.
    expect(staleNote(snapshot({ by: "jesseh", staledBy: "sam" }))).toBe(
      "sam has edited the plan since",
    );

    // A signature that still covers what the plan holds says nothing at all —
    // which is every signature in a project one person writes to.
    expect(staleNote(snapshot({ by: "jesseh" }))).toBeNull();
    expect(staleNote(snapshot())).toBeNull();
    expect(staleNote(undefined)).toBeNull();

    // Stale is about the PLAN moving, never about who approved: the signature
    // still reads as the signer's, so the two never get confused.
    const staled = snapshot({ by: "jesseh", staledBy: "sam" });
    expect(signatureLabel(staled)).toBe("jesseh");
  });

  it("resp-h4rf4g: the Changes page renders it beside the signature", () => {
    const src = fileURLToPath(new URL("../src/", import.meta.url));
    const page = readFileSync(src + "special/ChangesPage.tsx", "utf8");

    // Gated on the name, so the badge cannot appear without saying by whom.
    expect(page).toContain("signedOff?.staledBy && (");
    expect(page).toContain("Needs re-signing — {staleNote(signedOff)}");
    // Not the approved hue: a stale signature is not a signature on this plan.
    expect(page).toContain("text-amber-700 dark:text-amber-400");
    // And the button that resolves it says why it is being asked for.
    expect(page).toContain("Approve the plan as it stands — ${staleNote(signedOff)}");
  });

  it("resp-z5zzvj: the Changes page shows the signature, badge and tooltip", () => {
    const src = fileURLToPath(new URL("../src/", import.meta.url));
    const page = readFileSync(src + "special/ChangesPage.tsx", "utf8");
    expect(page).toContain("signatureLabel(signedOff)");
    expect(page).toContain("as ${signedOff.onBehalfOf}'s proxy");

    // And the type it reads carries all three facts the Rust ledger records.
    const ledger = readFileSync(src + "ledger.ts", "utf8");
    for (const field of ["by?: string", "onBehalfOf?: string", "staledBy?: string"]) {
      expect(ledger, field).toContain(field);
    }
  });
});
