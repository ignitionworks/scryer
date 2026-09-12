/**
 * The theme hook — a host's palette and type stack, in place of the default.
 *
 * This does not paint anything itself. `theme.ts` already turns a role map
 * into CSS custom properties (`--color-blue-500`, and the surface/text/border
 * tokens derived from the neutral ramp), and every component already reads
 * those; so a host's palette is applied by handing `applyTheme` a config, not
 * by touching a single component. That is the whole reason layout survives a
 * host theme: nothing here knows what a button looks like.
 *
 * Two things the desktop path does that this one must not. It SAVES — the
 * settings panel writes the user's choice to localStorage — and a host's theme
 * is the host's preference, not the user's, so nothing here persists. And it
 * speaks in the app's own role tokens (`zinc`, `blue`, `amber`); a host may use
 * those or the semantic names the theme's role table already publishes
 * (`neutral`, `accent`, `modified`), since a host thinks in "danger", not in
 * "red".
 */

import {
  applyTheme,
  DEFAULT_THEME,
  loadTheme,
  PALETTE_ROLE_KEYS,
  PALETTES,
  SHADES,
  THEME_ROLES,
  type PaletteName,
  type PaletteRole,
  type PaletteValues,
  type ThemeConfig,
} from "../theme";
import type { HostRamp, HostTheme, ResolvedHostTheme } from "./types";

/** Semantic name → role token, read off the theme's own role table so the two
 *  can't drift. First label wins (two roles share "Accent"; the primary one is
 *  the one a host means), and the reserved rows contribute nothing. */
const ROLE_ALIASES: Record<string, PaletteRole> = (() => {
  const out: Record<string, PaletteRole> = {};
  for (const role of THEME_ROLES) {
    const alias = role.label.toLowerCase();
    if (alias === "unused" || alias in out) continue;
    out[alias] = role.key;
  }
  return out;
})();

/** The custom properties the type stack sets. Tailwind's own font variables:
 *  every `font-sans`/`font-mono` utility already resolves through them. */
const FONT_SANS = "--font-sans";
const FONT_MONO = "--font-mono";

/** Resolve a host's key to a role, or null when it names neither a role token
 *  nor a semantic name. */
function roleOf(key: string): PaletteRole | null {
  const k = key.trim().toLowerCase();
  if ((PALETTE_ROLE_KEYS as string[]).includes(k)) return k as PaletteRole;
  return ROLE_ALIASES[k] ?? null;
}

/** A host's own ramp, once it's known to carry every shade. A partial ramp is
 *  refused rather than filled in: half a ramp paints unreadable chrome. */
function rampOf(value: HostRamp): PaletteValues | null {
  const out = {} as PaletteValues;
  for (const shade of SHADES) {
    const hex = value[shade];
    if (typeof hex !== "string" || hex.trim() === "") return null;
    out[shade] = hex.trim();
  }
  return out;
}

/** What a host's spec comes to, without applying any of it. */
export function resolveHostTheme(spec: HostTheme): ResolvedHostTheme {
  const theme: ThemeConfig = { ...DEFAULT_THEME, offsets: {} };
  const ramps: Record<string, PaletteValues> = {};
  const ignored: string[] = [];

  if (spec.mode) theme.colorMode = spec.mode;

  for (const [key, value] of Object.entries(spec.palette ?? {})) {
    const role = roleOf(key);
    if (!role) {
      ignored.push(key);
      continue;
    }
    if (typeof value === "string") {
      // A built-in palette, by name. An unknown name paints nothing, so it is
      // reported rather than left to resolve to undefined at apply time.
      if (!(value in PALETTES)) {
        ignored.push(key);
        continue;
      }
      theme[role] = value as PaletteName;
      continue;
    }
    const ramp = rampOf(value);
    if (!ramp) {
      ignored.push(key);
      continue;
    }
    // The host's ramp joins the built-ins under a name only this role uses, so
    // `applyTheme` selects it exactly as it selects "azure".
    const name = `host:${role}`;
    ramps[name] = ramp;
    theme[role] = name as PaletteName;
  }

  const fonts: Record<string, string> = {};
  const stack = spec.fontStack;
  if (typeof stack === "string") {
    if (stack.trim()) fonts[FONT_SANS] = stack.trim();
  } else if (stack) {
    if (stack.sans?.trim()) fonts[FONT_SANS] = stack.sans.trim();
    if (stack.mono?.trim()) fonts[FONT_MONO] = stack.mono.trim();
  }

  return { theme, ramps, fonts, ignored };
}

/** The font properties currently set by a host, so the next apply — or the
 *  handback — removes exactly what it put there and nothing else. */
let hostFonts: string[] = [];
let hostThemed = false;

/** Whether a host theme is in force. The desktop app never sets one. */
export function isHostThemed(): boolean {
  return hostThemed;
}

/**
 * Apply a host's palette and type stack in place of the default theme.
 *
 * Persists NOTHING: `applyTheme` paints, `saveTheme` is never called, and the
 * user's own stored theme is left exactly as it was, so unmounting the host
 * hands back the preference they had.
 */
export function applyHostTheme(spec: HostTheme): ResolvedHostTheme {
  const resolved = resolveHostTheme(spec);
  for (const [name, ramp] of Object.entries(resolved.ramps)) PALETTES[name] = ramp;
  applyTheme(resolved.theme);
  setFonts(resolved.fonts);
  hostThemed = true;
  return resolved;
}

/** Hand the theme back to the app's own stored preference — the state the
 *  desktop app boots into. Reads storage; still writes nothing. */
export function clearHostTheme(): void {
  setFonts({});
  applyTheme(loadTheme());
  hostThemed = false;
}

function setFonts(fonts: Record<string, string>): void {
  const root = document.documentElement;
  for (const prop of hostFonts) root.style.removeProperty(prop);
  for (const [prop, value] of Object.entries(fonts)) root.style.setProperty(prop, value);
  hostFonts = Object.keys(fonts);
}
