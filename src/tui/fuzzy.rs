//! Fuzzy matching for the app palette.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub score: i32,
    pub positions: Vec<usize>,
}

const UNMATCHED: i32 = i32::MIN / 8;

/// Scores `query` against `target` as a case-insensitive subsequence.
///
/// `None` means the query is not a subsequence of the target at all. Higher
/// scores mean a tighter match: an exact prefix beats a word-start match,
/// which beats characters scattered through the name.
pub fn score(query: &str, target: &str) -> Option<Hit> {
    let query: Vec<char> = query.chars().collect();
    let haystack: Vec<char> = target.chars().collect();
    if query.is_empty() {
        return Some(Hit {
            score: 0,
            positions: Vec::new(),
        });
    }
    if query.len() > haystack.len() {
        return None;
    }

    let needle = lowered(&query);
    let stack = lowered(&haystack);
    let rows = query.len();
    let cols = haystack.len();
    let mut best = vec![vec![UNMATCHED; cols]; rows];
    let mut from = vec![vec![usize::MAX; cols]; rows];

    for (col, &lower) in stack.iter().enumerate() {
        if lower != needle[0] {
            continue;
        }
        best[0][col] =
            2 + boundary(&haystack, col) + case_bonus(query[0], haystack[col]) - lead(col);
    }

    for row in 1..rows {
        for col in 0..cols {
            if stack[col] != needle[row] {
                continue;
            }
            let mut top = UNMATCHED;
            let mut top_col = usize::MAX;
            for (prev, &previous) in best[row - 1].iter().enumerate().take(col) {
                if previous == UNMATCHED {
                    continue;
                }
                let mut candidate =
                    previous + 2 + boundary(&haystack, col) + case_bonus(query[row], haystack[col]);
                if prev + 1 == col {
                    candidate += 8;
                }
                if candidate > top {
                    top = candidate;
                    top_col = prev;
                }
            }
            if top_col != usize::MAX {
                best[row][col] = top;
                from[row][col] = top_col;
            }
        }
    }

    let mut tail = usize::MAX;
    let mut highest = UNMATCHED;
    for (col, &score) in best[rows - 1].iter().enumerate() {
        if score > highest {
            highest = score;
            tail = col;
        }
    }
    if tail == usize::MAX {
        return None;
    }

    let mut positions = Vec::with_capacity(rows);
    let mut col = tail;
    for row in (0..rows).rev() {
        positions.push(col);
        col = from[row][col];
    }
    positions.reverse();

    let mut score = highest;
    if positions[0] == 0 {
        score += 12;
        if positions.iter().copied().eq(0..rows) {
            score += 8;
        }
    }
    Some(Hit { score, positions })
}

fn lowered(chars: &[char]) -> Vec<char> {
    chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect()
}

fn boundary(haystack: &[char], at: usize) -> i32 {
    if at == 0 {
        return 14;
    }
    let previous = haystack[at - 1];
    let current = haystack[at];
    if previous == ' ' || previous == '-' || previous == '_' || previous == '.' || previous == '&' {
        10
    } else if previous.is_lowercase() && current.is_uppercase() {
        8
    } else {
        0
    }
}

fn case_bonus(query: char, target: char) -> i32 {
    if query == target {
        1
    } else {
        0
    }
}

fn lead(col: usize) -> i32 {
    (col / 2).min(6) as i32
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn exact_prefix_beats_a_late_match() {
        let prefix = score("net", "Netflix").unwrap();
        let late = score("net", "Planet Earth").unwrap();
        assert!(prefix.score > late.score);
        assert_eq!(prefix.positions, vec![0, 1, 2]);
    }

    #[test]
    fn matches_are_case_insensitive() {
        assert!(score("yt", "YouTube").is_some());
        assert!(score("ÄK", "Äkäinen").is_some());
    }

    #[test]
    fn word_starts_are_marked() {
        let hit = score("tv", "Apple TV").unwrap();
        assert_eq!(hit.positions, vec![6, 7]);
    }

    #[test]
    fn rejects_missing_characters_and_long_queries() {
        assert!(score("zzz", "Netflix").is_none());
        assert!(score("netflix plus", "Netflix").is_none());
    }

    #[test]
    fn empty_query_matches_everything() {
        let hit = score("", "Netflix").unwrap();
        assert_eq!(hit.score, 0);
        assert!(hit.positions.is_empty());
    }

    #[test]
    fn positions_follow_the_matched_characters() {
        let hit = score("nflx", "Netflix").unwrap();
        assert_eq!(hit.positions, vec![0, 3, 4, 6]);
    }
}
