//! Vendored tree-sitter Clojure grammar.
//!
//! Exposes the generated parser as a [`LanguageFn`], matching the shape every
//! other grammar crate in the workspace exports (`tree_sitter_rust::LANGUAGE`
//! and kin), so `LANGUAGE.into()` yields a `tree_sitter::Language` for
//! whichever `tree-sitter` version the workspace resolves.
//!
//! See README.md for why the grammar is vendored rather than depended on.

use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_clojure() -> *const ();
}

/// The tree-sitter [`LanguageFn`] for the Clojure grammar.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_clojure) };

/// Upstream's highlight query, verbatim. It covers literals, comments and
/// quasiquotation only — with `defn` being a macro rather than syntax, there is
/// no node for a query to call a keyword. Callers wanting form highlighting
/// concatenate their own text-predicate query on top, the way the TypeScript
/// config layers on the JavaScript one.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammar-src/queries/highlights.scm");

#[cfg(test)]
mod tests {
    #[test]
    fn grammar_loads_and_parses() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("clojure grammar loads");
        let tree = parser.parse("(defn double [x] (* x 2))\n", None).unwrap();
        assert!(!tree.root_node().has_error());
        assert_eq!(tree.root_node().kind(), "source");
    }
}
