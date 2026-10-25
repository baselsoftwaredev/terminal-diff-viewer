//! Small, dependency-free path ranking and literal in-file search.

pub fn fuzzy_score(path: &str, query: &str) -> Option<i64> {
    let lower = path.to_lowercase();
    let basename = lower.rsplit('/').next().unwrap_or(&lower);
    let mut score = 0;
    for term in query.split_whitespace() {
        let term = term.to_lowercase();
        if basename == term {
            score += 10_000;
        } else if basename.starts_with(&term) {
            score += 5_000;
        } else if basename.contains(&term) {
            score += 3_000;
        } else if lower.contains(&term) {
            score += 1_000;
        }
        let mut chars = path.char_indices().flat_map(|(position, original)| {
            original
                .to_lowercase()
                .map(move |folded| (position, original, folded))
        });
        let mut previous = None;
        for wanted in term.chars() {
            let (position, matched, _) = chars.find(|(_, _, folded)| *folded == wanted)?;
            let before = path[..position].chars().next_back();
            score += 10;
            if before.is_none_or(|c| "/\\._- ".contains(c))
                || (matched.is_uppercase() && before.is_some_and(char::is_lowercase))
            {
                score += 30;
            }
            if previous == Some(position) {
                score += 40;
            }
            score -= position as i64;
            previous = Some(position + matched.len_utf8());
        }
    }
    Some(score)
}

pub fn occurrences<'a>(lines: impl Iterator<Item = &'a str>, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    lines
        .enumerate()
        .flat_map(|(row, text)| {
            text.match_indices(query)
                .map(move |(column, _)| (row, column))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranks_names_boundaries_and_multiple_terms() {
        assert!(
            fuzzy_score("src/ui.rs", "ui").unwrap()
                > fuzzy_score("build/ui_helpers.rs", "ui").unwrap()
        );
        assert!(fuzzy_score("src/LongFileName.cs", "lfn cs").is_some());
        assert!(fuzzy_score("src/LongFileName.cs", "lfn xyz").is_none());
        assert!(fuzzy_score("src/日本語.rs", "日本 rs").is_some());
        assert!(fuzzy_score("src/İstanbul.rs", "İs").is_some());
        assert!(fuzzy_score("src/ui.rs", "").is_some());
    }
    #[test]
    fn literal_search_tracks_each_occurrence_and_unicode_byte_boundaries() {
        assert_eq!(
            occurrences(["éé", "a.* a.*", "É"].into_iter(), "é"),
            [(0, 0), (0, 2)]
        );
        assert_eq!(occurrences(["a.* a.*"].into_iter(), ".*"), [(0, 1), (0, 5)]);
        assert!(occurrences(["abc"].into_iter(), "").is_empty());
    }
}
