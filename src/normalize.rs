//! Search normalization: case / punctuation / whitespace insensitive matching
//! across titles and alt-titles in all languages.

/// Lowercase, keep alphanumerics + spaces, collapse whitespace.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true; // skip leading spaces
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        } else if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        }
        // any other char (punctuation, symbols) is dropped
    }
    if prev_space {
        out.pop(); // trailing space
    }
    out
}

/// Match score of a normalized query against one normalized candidate:
/// 3 = exact, 2 = prefix, 1 = substring, 0 = no match.
pub fn score(nq: &str, candidate: &str) -> u8 {
    if nq.is_empty() || candidate.is_empty() {
        return 0;
    }
    if candidate == nq {
        3
    } else if candidate.starts_with(nq) {
        2
    } else if candidate.contains(nq) {
        1
    } else {
        0
    }
}

/// Best score of a raw query against several raw candidates (title + alt titles).
pub fn best_score(query: &str, candidates: &[&str]) -> u8 {
    let nq = norm(query);
    candidates.iter().map(|c| score(&nq, &norm(c))).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_basics() {
        assert_eq!(norm("  Berserk:  The  Golden Age! "), "berserk the golden age");
        assert_eq!(norm("Берсерк!!!"), "берсерк");
        assert_eq!(norm("ONE—PIECE (ワンピース)"), "onepiece ワンピース");
        assert_eq!(norm(""), "");
    }

    #[test]
    fn scoring() {
        assert_eq!(best_score("berserk", &["Berserk", "Berserk: Golden Age"]), 3);
        assert_eq!(best_score("ber", &["Berserk"]), 2);
        assert_eq!(best_score("serk", &["Berserk"]), 1);
        assert_eq!(best_score("naruto", &["Berserk"]), 0);
        assert_eq!(best_score("", &["Berserk"]), 0);
    }
}
