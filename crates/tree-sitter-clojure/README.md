# tree-sitter-clojure (vendored)

Generated parser for [tree-sitter-clojure][upstream] (`sogaiu`), vendored at
upstream commit `6248a3f3d3c76ec7d33c52e7553761facdcb4ab5` (the source of the
crates.io `tree-sitter-clojure` 0.1.0 release). Grammar is CC0 —
`grammar-src/COPYING.txt`.

## Why vendored instead of a crates.io dependency

`tree-sitter-clojure` 0.1.0 is the only grammar crate that takes a normal
dependency on `tree-sitter` itself (`^0.25.6`) rather than just the
version-stable `tree-sitter-language` shim. Because `tree-sitter` declares
`links = "tree-sitter"`, Cargo permits exactly one copy in the graph, so the
crate cannot coexist with this workspace's `tree-sitter = "0.26"`:

```
package `tree-sitter` links to the native library `tree-sitter`, but it
conflicts with a previous package which links to `tree-sitter` as well:
package `tree-sitter v0.25.6`
    ... which satisfies dependency `tree-sitter = "^0.25.6"` of package
        `tree-sitter-clojure v0.1.0`
help: try to adjust your dependencies so that only one package uses the
      `links = "tree-sitter"` value
```

That is a hard resolver failure, not a warning — it cannot be waived with a
feature flag, and the alternative (pinning the whole workspace back to
`tree-sitter` 0.25) would also drag `tree-sitter-highlight` backwards. So we
vendor `parser.c` and export the `LanguageFn` ourselves, which is the shape all
the other grammar crates already have.

**Do not replace this with the crates.io crate** unless upstream has dropped
its direct `tree-sitter` dependency.

## Updating

1. Fetch the upstream release: `cargo download tree-sitter-clojure` (or clone
   the repo and run `tree-sitter generate`).
2. Copy `src/parser.c`, `src/tree_sitter/parser.h`, and `src/node-types.json`
   into `grammar-src/src/`.
3. Update the commit hash above; run `cargo test -p tree-sitter-clojure`.

`node-types.json` is not compiled — it is kept as the reference for the node
kinds `scryer-extract`'s `collect_clojure` matches on (`list_lit`, `sym_lit`,
`sym_name`, `vec_lit`, `meta_lit`, `quoting_lit`, `dis_expr`, …).

[upstream]: https://github.com/sogaiu/tree-sitter-clojure
