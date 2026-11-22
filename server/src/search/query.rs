//! Extracts explicit named phrases from compound web queries for reference search.
use std::{collections::HashSet, sync::LazyLock};

use regex::Regex;

pub(super) fn named_phrases(query: &str) -> Vec<&str> {
    static QUOTED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#""([^"\s][^"]{0,75}[^"\s])""#).unwrap());
    static TITLE_CASE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b\p{Lu}[\p{L}\p{M}.-]*(?:[ \t]+\p{Lu}[\p{L}\p{M}.-]*){1,5}").unwrap()
    });
    let mut seen = HashSet::new();
    QUOTED
        .captures_iter(query)
        .filter_map(|capture| capture.get(1).map(|matched| matched.as_str()))
        .chain(TITLE_CASE.find_iter(query).map(|matched| matched.as_str()))
        .map(|phrase| phrase.trim_end_matches('.'))
        .filter(|phrase| (2..=6).contains(&phrase.split_whitespace().count()))
        .filter(|phrase| seen.insert(phrase.to_lowercase()))
        .take(5)
        .collect()
}
