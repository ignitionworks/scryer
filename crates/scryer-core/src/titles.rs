//! A responsibility's TITLE: its human name, and the rules that keep one
//! usable as a reference.
//!
//! An id identifies a claim; a title lets a person SAY which claim is meant.
//! The model already names its nodes and its groups, and a reader can point at
//! one without reciting a hash — a responsibility could not be pointed at at
//! all, so every reference to a claim fell back to `resp-a1b2c3`, which no
//! reader can hold in their head and no two people can say to each other.
//!
//! A title is worth having only while it still picks out ONE claim, so this
//! module holds the two rules that keep it one:
//!
//! * **SHAPE** — one or two words. Not a sentence: the statement is the
//!   sentence, and a title long enough to paraphrase it is a second, staler
//!   copy of the claim that drifts the moment the claim is reworded.
//! * **UNIQUENESS, node-scoped** — unique among the titles on ITS OWN node.
//!   The reference a reader is given is "the <title> responsibility of <node
//!   name>", so the node is already part of it; scoping the rule any wider
//!   would refuse `render` on two unrelated nodes for no reader's benefit.
//!
//! Comparison is by a NORMALISED form (case folded, whitespace collapsed),
//! because `Seeded Headings` and `seeded headings` are the same name said
//! twice, and a rule that let both onto one node would leave the reference
//! ambiguous exactly where it promised not to be.
//!
//! Nothing here decides WHEN a title is required — that belongs to the write
//! road, which is the only thing that knows whether a claim is new to it.

/// The most words a title may have. Two: enough for `seeded headings` or
/// `write-lock`, not enough to restate the claim.
pub const MAX_WORDS: usize = 2;

/// The most characters a title may have. A generous ceiling, there to catch a
/// statement pasted into the field rather than to police wording.
pub const MAX_CHARS: usize = 40;

/// A title reduced to what two titles are COMPARED by: case folded, outer
/// whitespace trimmed, inner runs of whitespace collapsed to one space.
///
/// Callers store the title as the author wrote it — the capitals and spacing
/// are theirs — and compare through here.
pub fn normalize(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `title` is a well-shaped title, or why it is not.
///
/// The error is the sentence a caller is shown, so it says what was wrong AND
/// what a good title looks like: a refusal that only says "invalid" earns a
/// second call that guesses again.
pub fn check_shape(title: &str) -> Result<(), String> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(
            "a title may not be empty or only whitespace — give one or two words naming the \
             claim, e.g. \"seeded headings\" or \"write-lock\""
                .to_string(),
        );
    }
    if trimmed.chars().count() > MAX_CHARS {
        return Err(format!(
            "title {title:?} is {} characters, over the {MAX_CHARS} a title may have — a title \
             NAMES the claim in a word or two; the statement is where it is said in full",
            trimmed.chars().count()
        ));
    }
    let words = trimmed.split_whitespace().count();
    if words > MAX_WORDS {
        return Err(format!(
            "title {title:?} is {words} words, over the {MAX_WORDS} a title may have — a title \
             NAMES the claim so a person can say which one is meant; the statement is where it \
             is said in full. Try the one or two words you would use out loud."
        ));
    }
    // A title is spoken and read, so the markup a statement carries would be
    // read out as itself. Caught here rather than silently stripped: a title
    // stripped behind the author's back is not the title they will look for.
    if let Some(bad) = trimmed.chars().find(|c| "*_`[]()<>|".contains(*c)) {
        return Err(format!(
            "title {title:?} contains {bad:?} — a title is plain words, with no markup: it is \
             read aloud and written into prose as it stands"
        ));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(format!(
            "title {title:?} contains a control character — a title is plain words"
        ));
    }
    Ok(())
}

/// The title already on `node_name`'s claims that `title` would collide with,
/// ignoring the claim with id `own_id` (a retitle must not collide with itself).
///
/// `existing` is every (id, title) pair the node holds AFTER the write would
/// land, so a caller checks the shape it is about to store and not the one it
/// started from.
pub fn collision<'a>(
    title: &str,
    own_id: &str,
    existing: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<(String, String)> {
    let wanted = normalize(title);
    existing.into_iter().find_map(|(id, other)| {
        (id != own_id && normalize(other) == wanted).then(|| (id.to_string(), other.to_string()))
    })
}

/// Check every title on one node's claims: each well-shaped, and no two the
/// same. The whole node is checked at once because uniqueness is a property of
/// the SET, not of any one title — a write that adds two claims titled the same
/// is refused by this and by nothing else.
///
/// `claims` is the node's responsibilities as the write would leave them.
pub fn check_node<'a>(
    node_name: &str,
    claims: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Result<(), String> {
    let mut seen: Vec<(String, String, String)> = Vec::new(); // normalized, id, as written
    for (id, title) in claims {
        let Some(title) = title else { continue };
        check_shape(title).map_err(|e| format!("claim '{id}' on '{node_name}': {e}"))?;
        let key = normalize(title);
        if let Some((_, other_id, other_title)) = seen.iter().find(|(k, _, _)| *k == key) {
            return Err(format!(
                "'{node_name}' would hold two claims titled {title:?} — claim '{other_id}' \
                 already carries {other_title:?}, and a title is the reference a reader is \
                 given, so it has to pick out ONE claim on its node. Retitle one of them \
                 (claim '{id}' is the one this write names), or point at the existing claim by \
                 its id if you meant to edit it rather than add beside it."
            ));
        }
        seen.push((key, id.to_string(), title.to_string()));
    }
    Ok(())
}

/// EVERY ROAD'S CHECK, at the one seam every plan write passes through.
///
/// The rule is "nothing NEW joins the model unnamed, by any door", and a rule
/// written per door is a rule with as many holes as doors somebody adds later:
/// the first version of this enforced only `add_component` and left seven other
/// roads open, which is exactly the shape the claim was widened to close.
///
/// So it is asked ONCE, of the model a write is about to leave behind, against
/// the model it started from:
///
/// * a responsibility whose id is NOT already in `before` is NEW, and needs a
///   title — whichever tool put it there, including one written tomorrow;
/// * a responsibility already in `before` may have none, and a write that says
///   nothing about its title leaves it alone;
/// * every host whose claims this write touched keeps its titles well-shaped
///   and distinct from each other.
///
/// Hosts are nodes AND groups: a group holds claims exactly as a node does, so
/// a claim entering through `update_group` is a claim entering the model.
pub fn check_write(before: &crate::ScryModel, after: &crate::ScryModel) -> Result<(), String> {
    let known: std::collections::HashSet<&str> = hosts(before)
        .flat_map(|(_, claims)| claims.iter().map(|r| r.id.as_str()))
        .collect();
    for (name, claims) in hosts(after) {
        let mut touched = false;
        for claim in claims {
            if !known.contains(claim.id.as_str()) {
                touched = true;
                if claim
                    .title
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
                    .is_empty()
                {
                    return Err(required(&claim.id));
                }
            }
        }
        // A host this write did not add a claim to is not re-judged: a model
        // that already holds two claims under one title (nothing stops one
        // written before this rule) must not refuse every unrelated write
        // until somebody fixes it.
        if touched {
            check_node(
                name,
                claims.iter().map(|r| (r.id.as_str(), r.title.as_deref())),
            )?;
        }
    }
    Ok(())
}

/// Every claim-holding thing in the model, by the name a refusal should say.
fn hosts(model: &crate::ScryModel) -> impl Iterator<Item = (&str, &Vec<crate::Responsibility>)> {
    model
        .nodes
        .iter()
        .map(|n| (n.name.as_str(), &n.responsibilities))
        .chain(
            model
                .groups
                .iter()
                .map(|g| (g.name.as_str(), &g.responsibilities)),
        )
}

/// The sentence a write is refused with when a claim NEW to it carries no
/// title. One place, because every road says it and a reader who meets it on
/// one should recognise it on the next.
pub fn required(id: &str) -> String {
    format!(
        "claim '{id}' is new and names no `title` — a claim joining the model is given a human \
         name, one or two words, unique among the titles on its node, so a person can say which \
         claim is meant instead of reciting its id. Add `title` beside `statement`. (An \
         EXISTING claim may have none: those written before titles kept their id as the only \
         reference, and a write that does not name a title leaves them as they are.)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_folds_case_and_collapses_whitespace() {
        assert_eq!(normalize("  Seeded   Headings "), "seeded headings");
        assert_eq!(normalize("write-lock"), "write-lock");
    }

    #[test]
    fn a_title_is_one_or_two_words() {
        assert!(check_shape("write-lock").is_ok());
        assert!(check_shape("seeded headings").is_ok());
        let three = check_shape("the seeded headings").unwrap_err();
        assert!(three.contains("3 words"), "{three}");
    }

    #[test]
    fn an_empty_or_marked_up_title_is_refused() {
        assert!(check_shape("   ").unwrap_err().contains("may not be empty"));
        assert!(check_shape("**bold**").unwrap_err().contains("no markup"));
    }

    #[test]
    fn a_title_over_the_ceiling_is_refused_as_a_statement_in_the_field() {
        let long = "a".repeat(MAX_CHARS + 1);
        assert!(check_shape(&long).unwrap_err().contains("characters"));
    }

    #[test]
    fn a_collision_is_found_across_case_and_spacing_but_never_with_itself() {
        let existing = vec![("resp-1", "seeded headings"), ("resp-2", "write-lock")];
        let hit = collision("Seeded   Headings", "resp-9", existing.clone());
        assert_eq!(hit, Some(("resp-1".into(), "seeded headings".into())));
        assert_eq!(collision("seeded headings", "resp-1", existing), None);
    }

    #[test]
    fn check_node_refuses_two_claims_with_one_title_and_names_both() {
        let err = check_node(
            "Change Statement",
            vec![
                ("resp-1", Some("seeded headings")),
                ("resp-2", Some("Seeded Headings")),
            ],
        )
        .unwrap_err();
        assert!(err.contains("resp-1"), "{err}");
        assert!(err.contains("resp-2"), "{err}");
        assert!(err.contains("Change Statement"), "{err}");
    }

    #[test]
    fn check_node_passes_untitled_claims_by() {
        // Legacy claims carry no title and a write that does not touch them
        // must not be refused on their account.
        assert!(check_node(
            "Change Statement",
            vec![
                ("resp-1", None),
                ("resp-2", None),
                ("resp-3", Some("titled"))
            ],
        )
        .is_ok());
    }
}
