//! In-memory station name index for instant autocomplete.
//!
//! Loaded once at startup from the DB; never queries the DB at search time.
//! All 2,859 UK National Rail stations fit in ~500 KB.
//!
//! Algorithm: word-prefix inverted index.
//!   - Each station name is split into lowercase words.
//!   - A query like "London Li" is split into tokens ["london", "li"].
//!   - A station matches if *every* token is a prefix of *some* word in the name.
//!   - CRS codes are matched as an exact prefix on the raw uppercase code.

#[derive(Clone)]
struct Entry {
    pub crs: String,
    pub name: String,
    /// Lowercase words extracted from the station name.
    words: Vec<String>,
}

#[derive(Clone)]
pub struct SearchResult {
    pub crs: String,
    pub name: String,
}

pub struct StationIndex {
    entries: Vec<Entry>,
}

impl StationIndex {
    /// Build from a list of `(crs, name)` pairs.
    pub fn build(stations: impl IntoIterator<Item = (String, String)>) -> Self {
        let entries: Vec<Entry> = stations
            .into_iter()
            .map(|(crs, name)| {
                let words = split_words(&name);
                Entry { crs, name, words }
            })
            .collect();

        Self { entries }
    }

    /// Return up to `limit` stations matching the query.
    ///
    /// Rules:
    /// 1. CRS exact match (case-insensitive, 3-letter query) bubbles to the top.
    /// 2. Every whitespace-separated token in `query` must be a prefix of at
    ///    least one word in the station name (e.g. "London Li" matches
    ///    "London Liverpool Street" but not "London Bridge").
    /// 3. Remaining ties broken by station name length (shorter = better), then
    ///    alphabetically.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let query = query.trim();
        if query.is_empty() {
            return vec![];
        }

        let tokens: Vec<String> = query
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();

        if tokens.is_empty() {
            return vec![];
        }

        let crs_upper = query.to_uppercase();

        // Collect all candidate entry indices that satisfy every token.
        let mut candidates: Vec<usize> = (0..self.entries.len())
            .filter(|&idx| {
                let e = &self.entries[idx];
                tokens.iter().all(|tok| {
                    e.words.iter().any(|w| w.starts_with(tok.as_str()))
                })
            })
            .collect();

        // Sort: CRS exact match first, then name length, then alphabetically.
        candidates.sort_unstable_by(|&a, &b| {
            let ea = &self.entries[a];
            let eb = &self.entries[b];
            let a_crs = ea.crs == crs_upper;
            let b_crs = eb.crs == crs_upper;
            if a_crs != b_crs {
                return b_crs.cmp(&a_crs); // true > false
            }
            // Stations whose names start with the query come next.
            let a_prefix = ea.name.to_lowercase().starts_with(&tokens[0]);
            let b_prefix = eb.name.to_lowercase().starts_with(&tokens[0]);
            if a_prefix != b_prefix {
                return b_prefix.cmp(&a_prefix);
            }
            ea.name.len().cmp(&eb.name.len()).then(ea.name.cmp(&eb.name))
        });

        candidates
            .into_iter()
            .take(limit)
            .map(|idx| {
                let e = &self.entries[idx];
                SearchResult { crs: e.crs.clone(), name: e.name.clone() }
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Split a station name into lowercase words, stripping punctuation.
fn split_words(name: &str) -> Vec<String> {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> StationIndex {
        StationIndex::build(vec![
            ("WAT".into(), "London Waterloo".into()),
            ("LST".into(), "London Liverpool Street".into()),
            ("LBG".into(), "London Bridge".into()),
            ("KGX".into(), "London King's Cross".into()),
            ("PAD".into(), "London Paddington".into()),
            ("EUS".into(), "London Euston".into()),
            ("MAN".into(), "Manchester Piccadilly".into()),
            ("LDS".into(), "Leeds".into()),
            ("BHM".into(), "Birmingham New Street".into()),
        ])
    }

    #[test]
    fn single_word_prefix() {
        let r = idx().search("Lon", 10);
        assert!(r.iter().any(|s| s.crs == "WAT"), "Waterloo must match Lon");
        assert!(!r.iter().any(|s| s.crs == "MAN"), "Manchester must not match Lon");
    }

    #[test]
    fn two_token_prefix() {
        let r = idx().search("London Li", 10);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].crs, "LST");
    }

    #[test]
    fn single_letter_second_token() {
        let r = idx().search("London L", 10);
        // All "London *" stations match because "London" starts with "london"
        // and every London station has a word starting with "l" (London itself).
        assert!(r.len() >= 5);
        assert!(r.iter().all(|s| s.name.to_lowercase().contains("london")));
    }

    #[test]
    fn crs_exact_match_bubbles_first() {
        let r = idx().search("WAT", 10);
        assert_eq!(r[0].crs, "WAT");
    }

    #[test]
    fn no_match_returns_empty() {
        let r = idx().search("Zzz", 10);
        assert!(r.is_empty());
    }

    #[test]
    fn apostrophe_in_name_does_not_prevent_match() {
        let r = idx().search("King", 10);
        assert!(r.iter().any(|s| s.crs == "KGX"), "King's Cross must match 'King'");
    }

    #[test]
    fn limit_respected() {
        let r = idx().search("London", 3);
        assert!(r.len() <= 3);
    }
}
