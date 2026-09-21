//! FULL-TEXT OCCURRENCES over the model's own prose — every place a word is
//! used, not the best places.
//!
//! `search_model`'s ranked search answers "which nodes are this task about":
//! fuzzy, scored, capped at fifty, and exactly right for orienting. It is the
//! wrong tool for "where is this word used", which is what a glossary sweep, a
//! read-across and a person asking after a term all need: those want EVERY
//! use, with no cap and no ranking, and they want the sentence so a reader can
//! judge the use without opening the node.
//!
//! So this is a different question with a different answer shape (L315 F,
//! resp-9rp8zq). One exact term, case-insensitively, whole-word by default —
//! because a sweep for `⟦fold⟧` that also reported "folded", "folding" and
//! "unfolded" is a sweep somebody has to filter by hand — with a flag for the
//! substring search when that is what was meant. Each hit names the element it
//! sits in BY ID and carries the sentence around the term, on the committed
//! layer, the planned layer, or both as asked.
//!
//! THE PROSE THIS READS is the three kinds the model authors: a claim's
//! statement, a description (a node's, a group's, a property's) and a
//! directive (a node's or a claim's). Not names, not technology badges, not
//! property labels — those are identifiers a reader searches for differently —
//! and not the source map or the boundaries, which are paths rather than
//! prose.

use serde::{Deserialize, Serialize};

use crate::{Group, Node, ScryModel};

/// One use of the term, in the element it sits in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Occurrence {
    /// `claim`, `description` or `directive` — which kind of prose this is.
    pub kind: &'static str,
    /// `committed` or `planned`.
    pub layer: &'static str,
    /// The element's own id: a responsibility id for a claim, a node or group
    /// id for a description or a node directive, `<owner>:<label>` for a
    /// property's description. A directive on a claim carries the claim's id.
    pub id: String,
    /// The node or group that holds it, when the element is not itself one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    /// The breadcrumb of the holding node or group — the PLACE, which is how a
    /// reader (and the read-across's summary) says where the uses are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Which text on the element: `statement`, `description`, or
    /// `directive 2` for the second of a directive list.
    pub field: String,
    /// The sentence the term sits in, as written.
    pub sentence: String,
}

/// The most of a sentence a hit carries. A claim's statement can be a long
/// clause with no full stop in it, and a hit is meant to be read at a glance:
/// past this the sentence is windowed around the term.
const SENTENCE_LIMIT: usize = 400;

/// Every use of `term` in the prose of the models given, in document order:
/// layer by layer as handed in, then node by node as the model stores them,
/// then field by field, then offset by offset. NO CAP and no ranking — the
/// caller asked for all of them, and the order is the model's own.
///
/// `whole_word` (the default at every caller) requires a non-word character on
/// each side of the match, so `fold` does not report `folded`. A word
/// character is a Unicode alphanumeric or `_`; a hyphen is a boundary, so
/// `read` is a whole word in `read-across`. A multi-word term (`relevant set`)
/// is bounded at its two ends, which is what makes the glossary's phrases
/// searchable at all.
pub fn find(
    layers: &[(&'static str, &ScryModel)],
    term: &str,
    whole_word: bool,
) -> Vec<Occurrence> {
    let needle = term.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (layer, model) in layers {
        for n in &model.nodes {
            let path = breadcrumb(model, &n.id);
            node_prose(
                &mut out,
                layer,
                model,
                n,
                path.as_deref(),
                &needle,
                whole_word,
            );
        }
        for g in &model.groups {
            let path = g
                .parent_node_id
                .as_deref()
                .and_then(|p| breadcrumb(model, p));
            group_prose(&mut out, layer, g, path.as_deref(), &needle, whole_word);
        }
    }
    out
}

/// The node's place, by name, parent-first — the same reading `search_model`
/// gives a hit. `None` for an id the layer does not hold.
fn breadcrumb(model: &ScryModel, node_id: &str) -> Option<String> {
    let by_id: std::collections::HashMap<&str, &Node> =
        model.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut names = Vec::new();
    let mut cur = by_id.get(node_id).copied();
    cur?;
    let mut guard = 0;
    while let Some(n) = cur {
        names.push(n.name.as_str());
        cur = n.parent_id.as_deref().and_then(|p| by_id.get(p).copied());
        guard += 1;
        if guard > 64 {
            break; // cycle guard, as every ancestor walk in the tree has
        }
    }
    names.reverse();
    Some(names.join(" / "))
}

fn node_prose(
    out: &mut Vec<Occurrence>,
    layer: &'static str,
    _model: &ScryModel,
    n: &Node,
    path: Option<&str>,
    needle: &str,
    whole_word: bool,
) {
    if let Some(d) = &n.description {
        push_all(out, needle, whole_word, d, || Occurrence {
            kind: "description",
            layer,
            id: n.id.clone(),
            host_id: None,
            path: path.map(str::to_string),
            field: "description".into(),
            sentence: String::new(),
        });
    }
    // The directive's PROSE only: a citation is an opaque external anchor id,
    // never the model's words, so a word search never reaches into one.
    for (i, d) in n.directives.iter().enumerate() {
        push_all(out, needle, whole_word, &d.text, || Occurrence {
            kind: "directive",
            layer,
            id: n.id.clone(),
            host_id: None,
            path: path.map(str::to_string),
            field: format!("directive {}", i + 1),
            sentence: String::new(),
        });
    }
    for p in &n.properties {
        if p.description.is_empty() {
            continue;
        }
        push_all(out, needle, whole_word, &p.description, || Occurrence {
            kind: "description",
            layer,
            id: format!("{}:{}", n.id, p.label),
            host_id: Some(n.id.clone()),
            path: path.map(str::to_string),
            field: "description".into(),
            sentence: String::new(),
        });
    }
    claims_prose(
        out,
        layer,
        &n.id,
        path,
        &n.responsibilities,
        needle,
        whole_word,
    );
}

fn group_prose(
    out: &mut Vec<Occurrence>,
    layer: &'static str,
    g: &Group,
    path: Option<&str>,
    needle: &str,
    whole_word: bool,
) {
    if let Some(d) = &g.description {
        push_all(out, needle, whole_word, d, || Occurrence {
            kind: "description",
            layer,
            id: g.id.clone(),
            host_id: None,
            path: path.map(str::to_string),
            field: "description".into(),
            sentence: String::new(),
        });
    }
    claims_prose(
        out,
        layer,
        &g.id,
        path,
        &g.responsibilities,
        needle,
        whole_word,
    );
}

fn claims_prose(
    out: &mut Vec<Occurrence>,
    layer: &'static str,
    host: &str,
    path: Option<&str>,
    claims: &[crate::Responsibility],
    needle: &str,
    whole_word: bool,
) {
    for r in claims {
        push_all(out, needle, whole_word, &r.statement, || Occurrence {
            kind: "claim",
            layer,
            id: r.id.clone(),
            host_id: Some(host.to_string()),
            path: path.map(str::to_string),
            field: "statement".into(),
            sentence: String::new(),
        });
        for (i, d) in r.directives.iter().enumerate() {
            push_all(out, needle, whole_word, &d.text, || Occurrence {
                kind: "directive",
                layer,
                id: r.id.clone(),
                host_id: Some(host.to_string()),
                path: path.map(str::to_string),
                field: format!("directive {}", i + 1),
                sentence: String::new(),
            });
        }
    }
}

/// One hit per OCCURRENCE, not per element: the same claim using a word three
/// times is three uses, and each carries its own sentence — which is the whole
/// point of reading the sentence rather than the element.
fn push_all(
    out: &mut Vec<Occurrence>,
    needle: &str,
    whole_word: bool,
    text: &str,
    make: impl Fn() -> Occurrence,
) {
    for at in matches(text, needle, whole_word) {
        let mut o = make();
        o.sentence = sentence_around(text, at, needle.len());
        out.push(o);
    }
}

/// The byte offsets of every match of `needle` (already lowercased) in `text`,
/// case-insensitively.
///
/// Lowercasing can change a string's LENGTH (`İ` is two bytes and lowercases
/// to three), so the offsets are found in the lowercased copy and mapped back
/// through a per-byte index rather than assumed equal. Prose with no such
/// character — every statement in the tree — takes the identity map.
fn matches(text: &str, needle: &str, whole_word: bool) -> Vec<usize> {
    let mut lower = String::with_capacity(text.len());
    // For each byte of `lower`, the byte offset in `text` it came from.
    let mut origin: Vec<usize> = Vec::with_capacity(text.len() + 1);
    for (i, ch) in text.char_indices() {
        for lc in ch.to_lowercase() {
            let before = lower.len();
            lower.push(lc);
            for _ in before..lower.len() {
                origin.push(i);
            }
        }
    }
    origin.push(text.len());

    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        if !whole_word || (is_boundary(&lower, start, true) && is_boundary(&lower, end, false)) {
            out.push(origin[start]);
        }
        // Advance by one CHARACTER so overlapping uses are all found.
        from = lower[start..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| start + i)
            .unwrap_or(lower.len());
        if from >= lower.len() {
            break;
        }
    }
    out
}

/// Whether the match edge at byte `at` sits on a word boundary. `before` asks
/// about the character ENDING at `at`; otherwise the one starting there.
fn is_boundary(s: &str, at: usize, before: bool) -> bool {
    let ch = if before {
        if at == 0 {
            return true;
        }
        s[..at].chars().next_back()
    } else {
        if at >= s.len() {
            return true;
        }
        s[at..].chars().next()
    };
    match ch {
        None => true,
        Some(c) => !(c.is_alphanumeric() || c == '_'),
    }
}

/// The sentence the match sits in. Sentence ends are `.`, `!`, `?` followed by
/// whitespace or the end of the text, and a newline; a run of prose with no
/// such mark is one sentence, windowed around the term when it is longer than
/// a reader wants at a glance.
fn sentence_around(text: &str, at: usize, len: usize) -> String {
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < at {
        let c = bytes[i];
        if c == b'\n' {
            start = i + 1;
        } else if matches!(c, b'.' | b'!' | b'?') {
            let next = bytes.get(i + 1).copied();
            if next.is_none_or(|n| n.is_ascii_whitespace()) {
                start = i + 1;
            }
        }
        i += 1;
    }
    let mut end = text.len();
    let mut j = at + len;
    while j < text.len() {
        let c = bytes[j];
        if c == b'\n' {
            end = j;
            break;
        }
        if matches!(c, b'.' | b'!' | b'?') {
            let next = bytes.get(j + 1).copied();
            if next.is_none_or(|n| n.is_ascii_whitespace()) {
                end = j + 1;
                break;
            }
        }
        j += 1;
    }
    let (start, end) = (floor_char(text, start), ceil_char(text, end));
    let sentence = text[start..end].trim();
    if sentence.chars().count() <= SENTENCE_LIMIT {
        return sentence.to_string();
    }
    // Too long to read: a window around the term, with the term whole.
    let rel = at.saturating_sub(start);
    let half = SENTENCE_LIMIT / 2;
    let lo = floor_char(&text[start..end], rel.saturating_sub(half));
    let hi = ceil_char(&text[start..end], (rel + len + half).min(end - start));
    format!("…{}…", text[start..end][lo..hi].trim())
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i.min(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(json: serde_json::Value) -> ScryModel {
        serde_json::from_value(json).unwrap()
    }

    fn fixture() -> ScryModel {
        model(serde_json::json!({
            "version": crate::SCRY_VERSION,
            "nodes": [{
                "id": "n1",
                "kind": "component",
                "name": "Yada",
                "description": "Holds the proxy. A proxy is not a Proxy Server.",
                "directives": ["must never proxy a token"],
                "properties": [
                    { "label": "proxyUrl", "description": "Where the proxy lives." }
                ],
                "responsibilities": [
                    { "id": "r1", "statement": "**Answers** as the proxy, and proxies nothing else" },
                    { "id": "r2", "statement": "**Refuses** a request with no proxy",
                      "directives": ["must log the proxy"] }
                ]
            }],
            "links": []
        }))
    }

    fn one_layer(m: &ScryModel, term: &str, whole: bool) -> Vec<Occurrence> {
        find(&[("planned", m)], term, whole)
    }

    /// resp-b00631 — a word search does not reach INTO a citation. An anchor id
    /// is an opaque external string, not the model's prose: the engine stores
    /// and answers it and interprets none of it, so a sweep over the team's
    /// words must not report a coincidence inside one. The directive's own
    /// words are still found, as they always were — the citation rides beside
    /// them without joining them.
    #[test]
    fn resp_b00631_a_word_search_never_reaches_into_a_citation() {
        let m = model(serde_json::json!({
            "version": crate::SCRY_VERSION,
            "nodes": [{
                "id": "n1",
                "kind": "component",
                "name": "Yada",
                // Every citation below spells the searched word, and none of
                // them is prose anybody wrote for a reader.
                "directives": [{ "text": "must never log a token",
                                 "cites": ["doc-proxy-anchor", "proxy"] }],
                "responsibilities": [
                    { "id": "r1", "statement": "**Answers** as the proxy",
                      "cites": ["proxy", "doc-proxy-first"],
                      "directives": [{ "text": "must stay stateless",
                                       "cites": ["proxy"] }] }
                ]
            }],
            "links": []
        }));
        let hits = one_layer(&m, "proxy", true);
        let shape: Vec<(&str, &str, &str)> = hits
            .iter()
            .map(|h| (h.kind, h.id.as_str(), h.field.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![("claim", "r1", "statement")],
            "only the ONE prose use is reported — six anchor ids spelling the \
             same word are not uses of it"
        );

        // And the directives' own words are still searchable: skipping the
        // citations did not skip the directive.
        let words = one_layer(&m, "stateless", true);
        assert_eq!(
            words
                .iter()
                .map(|h| (h.kind, h.field.as_str()))
                .collect::<Vec<_>>(),
            vec![("directive", "directive 1")]
        );
    }

    #[test]
    fn resp_9rp8zq_every_use_is_reported_with_its_id_and_its_sentence() {
        let m = fixture();
        let hits = one_layer(&m, "proxy", true);
        let shape: Vec<(&str, &str, &str)> = hits
            .iter()
            .map(|h| (h.kind, h.id.as_str(), h.field.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                // "Holds the proxy. A proxy is not a Proxy Server." uses it
                // three times, and "Proxy" in "Proxy Server" is one of them.
                ("description", "n1", "description"),
                ("description", "n1", "description"),
                ("description", "n1", "description"),
                ("directive", "n1", "directive 1"),
                ("description", "n1:proxyUrl", "description"),
                ("claim", "r1", "statement"),
                ("claim", "r2", "statement"),
                ("directive", "r2", "directive 1"),
            ],
            "every kind of prose, in the model's own order, one hit per use: {shape:?}"
        );
        // The SENTENCE around the term, not the whole text: the description
        // holds two sentences and the two uses report one each.
        assert_eq!(hits[0].sentence, "Holds the proxy.");
        assert_eq!(hits[1].sentence, "A proxy is not a Proxy Server.");
        assert_eq!(hits[2].sentence, "A proxy is not a Proxy Server.");
        assert_eq!(
            hits[5].sentence,
            "**Answers** as the proxy, and proxies nothing else"
        );
        assert_eq!(hits[5].host_id.as_deref(), Some("n1"), "and where it sits");
        assert_eq!(hits[5].path.as_deref(), Some("Yada"), "and its place");
    }

    #[test]
    fn resp_9rp8zq_whole_word_by_default_and_a_flag_for_substrings() {
        let m = fixture();
        // Whole-word: "proxies" and "proxyUrl" are not uses of "proxy".
        let whole = one_layer(&m, "proxy", true);
        assert_eq!(whole.len(), 8);
        assert!(
            whole.iter().all(|h| !h.sentence.contains("proxyUrl")),
            "the property LABEL is not prose and is never a hit"
        );
        // Case-insensitive either way: "Proxy" in "Proxy Server" is a use.
        assert!(whole.iter().any(|h| h.sentence.contains("Proxy Server")));

        // THE CASE THE DEFAULT EXISTS FOR: a sweep for `fold` must not report
        // `folded` and `unfolded`, and the flag is there for when it should.
        let mut m2 = fixture();
        m2.nodes[0].description = Some("The fold folded an unfolded claim.".into());
        assert_eq!(
            one_layer(&m2, "fold", true)
                .iter()
                .map(|h| h.sentence.as_str())
                .collect::<Vec<_>>(),
            vec!["The fold folded an unfolded claim."],
            "one use of the word itself, whole-word"
        );
        assert_eq!(
            one_layer(&m2, "fold", false).len(),
            3,
            "and all three when substrings are asked for"
        );
        // And a term nobody wrote finds nothing rather than everything.
        assert!(one_layer(&m, "widget", true).is_empty());
        assert!(one_layer(&m, "   ", true).is_empty());
    }

    #[test]
    fn resp_9rp8zq_a_multi_word_term_is_bounded_at_its_two_ends() {
        let m = model(serde_json::json!({
            "version": crate::SCRY_VERSION,
            "nodes": [{
                "id": "n1", "kind": "component", "name": "C",
                "description": "The relevant set is read. A relevant setting is not.",
                "responsibilities": []
            }],
            "links": []
        }));
        let hits = one_layer(&m, "relevant set", true);
        assert_eq!(hits.len(), 1, "the phrase, not its prefix: {hits:?}");
        assert_eq!(hits[0].sentence, "The relevant set is read.");
    }

    #[test]
    fn resp_9rp8zq_both_layers_are_answered_when_asked_and_told_apart() {
        let committed = fixture();
        let mut planned = fixture();
        planned.nodes[0].responsibilities[0].statement =
            "**Answers** as the proxy, twice over".into();

        let both = find(
            &[("committed", &committed), ("planned", &planned)],
            "proxy",
            true,
        );
        assert_eq!(both.len(), 16, "eight uses in each layer");
        assert!(both[..8].iter().all(|h| h.layer == "committed"));
        assert!(both[8..].iter().all(|h| h.layer == "planned"));
        let planned_claim = both
            .iter()
            .find(|h| h.layer == "planned" && h.id == "r1")
            .unwrap();
        assert_eq!(
            planned_claim.sentence,
            "**Answers** as the proxy, twice over"
        );

        // One layer alone answers only that layer.
        assert_eq!(find(&[("committed", &committed)], "proxy", true).len(), 8);
    }

    #[test]
    fn resp_9rp8zq_a_long_clause_with_no_full_stop_is_windowed_around_the_term() {
        let filler = "and on ".repeat(120);
        let mut m = fixture();
        m.nodes[0].responsibilities[0].statement = format!("**Answers** {filler}proxy {filler}end");
        let hits = one_layer(&m, "proxy", true);
        let claim = hits.iter().find(|h| h.id == "r1").unwrap();
        assert!(
            claim.sentence.chars().count() < 460,
            "a hit is readable at a glance: {} chars",
            claim.sentence.chars().count()
        );
        assert!(claim.sentence.contains("proxy"), "with the term in it");
        assert!(claim.sentence.starts_with('…') && claim.sentence.ends_with('…'));
    }
}
