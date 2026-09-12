# Mounting this UI in a host

`App` takes an optional `hostBridge` ref (a callback or a React ref), handed the bridge on
mount and null on unmount; `useHostBridge()` returns it from inside. The desktop app passes none.

```tsx
const bridge = useRef<HostBridge | null>(null);
<App hostBridge={bridge} />;

// Go somewhere — lands the user exactly where clicking would: expands the tree to the
// node, flashes a claim's row, pins the Changes page. `{ok: false, reason}` if unknown.
bridge.current.navigateTo({ kind: "node", id: "node-5b6qhs" }); // group | claim | change
bridge.current.navigateTo({ kind: "view", id: "map" });         // wiki | map | inbox

// Follow along — on every change of selected node, group, special page or view, never on a
// re-render landing in the same place; called at once with the current selection, so a panel
// mounting late is not blank. kind: node | group | special | none. view: wiki | map.
const off = bridge.current.onSelectionChange(({ kind, id, view }) => …);

// Wear your own theme — palette and type stack through the theme's own custom properties,
// so layout is untouched; persists nothing, and clearHostTheme() hands it back. Roles:
// neutral accent added modified drift danger agent syntax, or the tokens zinc blue emerald
// amber orange red violet indigo cyan slate teal; each value is a palette name or your own
// ramp keyed "50"…"950". What you leave out keeps the default; what it can't use, `ignored`.
bridge.current.applyHostTheme({
  palette: { accent: "violet", danger: "rose" },
  fontStack: { sans: "Inter, sans-serif", mono: "JetBrains Mono, monospace" },
  mode: "dark", // "light" | "dark" | "system"
});
```
