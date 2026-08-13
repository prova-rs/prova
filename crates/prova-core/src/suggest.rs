//! "Did you mean" — one proximity policy for every closed key set prova refuses.
//!
//! Two layers hold the same line: the manifest (`deny_unknown_fields` on every section) and the
//! DSL (`prova.test`'s `opts`, `suite.config`). They must not drift on what counts as a typo —
//! an author who learns the manifest's phrasing should read the DSL's the same way — so the
//! threshold lives here once rather than in each layer.

/// Levenshtein distance, iterative two-row. Small enough not to warrant a dependency.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The closest known key to `field`, if one is close enough to be worth naming.
///
/// No future version adds a key one edit away from an existing one, so proximity is proof of a
/// typo regardless of what the caller declares — which is why this needs no version context.
/// The threshold scales with length so short keys do not match everything.
pub fn nearest<'a>(field: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (field.chars().count() / 3).clamp(1, 3);
    candidates
        .map(|c| (edit_distance(field, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_near_miss_is_named_and_a_far_one_is_not() {
        let keys = ["timeout", "tags", "requires"];
        // A transposition inside a long key is two edits, and a 7-character key tolerates two.
        assert_eq!(nearest("tiemout", keys.into_iter()), Some("timeout"));
        // A short key tolerates only one, deliberately: at four characters, two edits reach
        // several unrelated words, and a confident wrong suggestion is worse than none —
        // `tgas` therefore gets the accepted-set listing instead of a guess at `tags`.
        assert_eq!(nearest("tag", keys.into_iter()), Some("tags"));
        assert_eq!(nearest("tgas", keys.into_iter()), None);
        // Not a typo of anything here — naming a "nearest" key would send the author to the
        // wrong fix, so the caller falls back to listing the accepted set.
        assert_eq!(nearest("parallelism", keys.into_iter()), None);
    }
}
