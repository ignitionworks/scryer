/**
 * Speaking the engine service's command shapes, so the app does not have to.
 *
 * The desktop's `invoke` and the service's HTTP surface answer two of the same
 * commands differently. `read_planned` is a bare string to the desktop and
 * `{revision, data}` to the service; `write_planned` takes no revision and
 * answers nothing to the desktop, takes `baseRevision` and answers `{revision}`
 * to the service. The difference exists for a reason — the service serializes
 * concurrent writers and the desktop assumes it is alone — so neither side is
 * wrong, and the translation has to live somewhere.
 *
 * It lives HERE, as a function a host installs into the shim it already needs.
 * Thirteen files under `src/` import `invoke` from `@tauri-apps/api/core`
 * directly, so there is no runtime seam to intercept and a mounting host is
 * already aliasing that module (`export-viewer/vite.config.ts` and
 * `demo/tauri-stub.ts` both do). Wrapping the transport costs zero edits to the
 * app; adding a seam would cost thirteen.
 *
 *     export const invoke = serviceInvoke(myTransport);
 *
 * Sending the base revision is OFF by default, and that is a real decision
 * rather than caution. The revision is the service's optimistic-concurrency
 * guard, and turning it on is strictly better for correctness — but the app's
 * plan save is `invoke("write_planned", …).catch(() => {})`, which SWALLOWS
 * failures, so a refused write would silently drop the user's edit. With
 * `optimistic` on, this adapter therefore owns the refusal: re-read, retry
 * once, and tell the host a conflict happened. Re-applying the user's edit onto
 * the plan that won is a different job, and it belongs to the app's storage
 * layer, not to a translation.
 */

/** A host's way of performing one command. The same shape as Tauri's `invoke`,
 *  because that is what it stands in for. */
export type HostInvoke = (
  command: string,
  args?: Record<string, unknown>,
) => Promise<unknown>;

export interface ServiceInvokeOptions {
  /** Send `baseRevision` on plan writes, so the service refuses one whose base
   *  has moved on. Default false — see the note above on what a refusal costs
   *  the app today. */
  optimistic?: boolean;
  /** Called when a write was refused and retried, with the project it was for.
   *  A host's cue to say "someone else changed the plan"; the retry has already
   *  happened by the time this runs. */
  onConflict?: (project: string) => void;
}

/** Wrap a service-shaped transport so the app sees the shapes it expects. */
export function serviceInvoke(
  transport: HostInvoke,
  options: ServiceInvokeOptions = {},
): HostInvoke {
  // One revision per project, not one for the app: a host may hold several
  // open at once, and a write must carry the base its OWN project was read at.
  const revisions = new Map<string, string>();

  const read = async (command: string, args?: Record<string, unknown>) => {
    const answer = await transport(command, args);
    const { revision, data } = plannedRead(answer);
    if (revision) revisions.set(projectOf(args), revision);
    return data;
  };

  const write = async (args?: Record<string, unknown>) => {
    const project = projectOf(args);
    const send = async (base: string | undefined) => {
      const answer = await transport("write_planned", base ? { ...args, baseRevision: base } : args);
      const revision = revisionOf(answer);
      if (revision) revisions.set(project, revision);
      return undefined;
    };

    if (!options.optimistic) return send(undefined);

    try {
      return await send(revisions.get(project));
    } catch (e) {
      if (!isStaleRevision(e)) throw e;
      // Somebody wrote in between. Re-read — which is also what refreshes the
      // revision — then write once more against what is actually current.
      await read("read_planned", args).catch(() => undefined);
      options.onConflict?.(project);
      return send(revisions.get(project));
    }
  };

  return (command, args) => {
    if (command === "read_planned") return read(command, args);
    if (command === "write_planned") return write(args);
    // Everything else is the same on both sides and is not this adapter's
    // business — it must arrive at the transport exactly as the app sent it.
    return transport(command, args);
  };
}

/** The project a command is about, normalized so `project:/x` and `/x` — both
 *  of which the app uses — key the same revision. */
function projectOf(args?: Record<string, unknown>): string {
  for (const key of ["refStr", "cwd", "projectPath", "project"]) {
    const value = args?.[key];
    if (typeof value === "string" && value !== "") {
      return value.startsWith("project:") ? value.slice("project:".length) : value;
    }
  }
  return "";
}

/** The service's `read_planned` answer, tolerating a transport that already
 *  returns the desktop's bare string (a host part-way through adopting this). */
function plannedRead(answer: unknown): { revision?: string; data: string } {
  if (typeof answer === "string") return { data: answer };
  if (answer && typeof answer === "object") {
    const { revision, data } = answer as { revision?: unknown; data?: unknown };
    if (typeof data === "string") {
      return { revision: typeof revision === "string" ? revision : undefined, data };
    }
  }
  throw new Error("read_planned did not answer with the plan's text");
}

function revisionOf(answer: unknown): string | undefined {
  if (answer && typeof answer === "object") {
    const { revision } = answer as { revision?: unknown };
    if (typeof revision === "string") return revision;
  }
  return undefined;
}

/** Whether a refusal is the service saying the plan moved on. Hosts surface
 *  errors differently — a thrown object, a thrown string, an Error with the
 *  body in its message — so all three are recognised rather than one. */
export function isStaleRevision(error: unknown): boolean {
  const kind = (error as { error?: { kind?: unknown }; kind?: unknown } | null)?.error?.kind
    ?? (error as { kind?: unknown } | null)?.kind;
  if (kind === "staleRevision") return true;
  const text =
    typeof error === "string" ? error : error instanceof Error ? error.message : "";
  return text.includes("staleRevision");
}
