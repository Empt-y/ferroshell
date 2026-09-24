//! Fuzzy matching for launcher search. Higher scores are better; `None` means no match.
//!
//! Tiers (so a better kind of match always wins):
//! - 1000+ the text starts with the query ("fire" → Firefox)
//! - 800+  the query is the words' initials ("vsc" → Visual Studio Code)
//! - 700+  a word starts with the query ("code" → Visual Studio Code)
//! - 500+  the query appears somewhere ("ox" → Firefox)
//! - 100+  the query's letters appear in order ("fx" → Firefox), scored by how tight

/// Lowercase and drop common Latin accents, so "cafe" finds "Café".
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .collect()
}

/// Indices (in `chars`) where a word starts: after a separator, or at a lower→upper
/// camel-case change in the original text.
fn word_starts(original: &[char]) -> Vec<bool> {
    let mut starts = vec![false; original.len()];
    for i in 0..original.len() {
        starts[i] = i == 0
            || !original[i - 1].is_alphanumeric()
            || (original[i - 1].is_lowercase() && original[i].is_uppercase())
            || (original[i - 1].is_alphabetic() && original[i].is_numeric());
    }
    starts
}

pub fn score(query: &str, text: &str) -> Option<u32> {
    let q: Vec<char> = fold(query.trim()).chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return None;
    }
    let original: Vec<char> = text.chars().collect();
    let t: Vec<char> = original.iter().map(|c| fold(&c.to_string()).chars().next().unwrap_or(*c)).collect();
    let starts = word_starts(&original);
    let qs: String = q.iter().collect();
    let ts: String = t.iter().collect();
    let len_penalty = (t.len().saturating_sub(q.len()) as u32).min(99);

    if ts.replace(' ', "").starts_with(&qs) && t.first() == q.first() {
        return Some(1000 + 99 - len_penalty);
    }
    let initials: String = t.iter().zip(&starts).filter(|(c, s)| **s && c.is_alphanumeric()).map(|(c, _)| *c).collect();
    if q.len() >= 2 && initials.starts_with(&qs) {
        return Some(800 + 99 - len_penalty);
    }
    if let Some(pos) = find_at_word_start(&t, &starts, &q) {
        return Some(700 + 99u32.saturating_sub(pos as u32));
    }
    if let Some(pos) = ts.find(&qs) {
        return Some(500 + 99u32.saturating_sub(ts[..pos].chars().count() as u32));
    }
    subsequence(&t, &starts, &q).map(|s| 100 + s.min(399))
}

fn find_at_word_start(t: &[char], starts: &[bool], q: &[char]) -> Option<usize> {
    (0..t.len()).find(|&i| starts[i] && t[i..].starts_with(q))
}

/// Greedy in-order match rewarding consecutive letters and word starts.
fn subsequence(t: &[char], starts: &[bool], q: &[char]) -> Option<u32> {
    let mut score: i64 = 0;
    let mut ti = 0;
    let mut prev: Option<usize> = None;
    for &qc in q {
        let found = (ti..t.len()).find(|&i| t[i] == qc)?;
        score += 10;
        if prev.is_some_and(|p| p + 1 == found) {
            score += 15;
        }
        if starts[found] {
            score += 20;
        }
        if let Some(p) = prev {
            score -= (found - p - 1).min(10) as i64;
        }
        prev = Some(found);
        ti = found + 1;
    }
    Some(score.max(1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_order_matches_sensibly() {
        let prefix = score("fire", "Firefox").unwrap();
        let initials = score("vsc", "Visual Studio Code").unwrap();
        let word = score("code", "Visual Studio Code").unwrap();
        let inner = score("fox", "Firefox").unwrap();
        let loose = score("ffx", "Firefox").unwrap();
        assert!(prefix > initials && initials > word && word > inner && inner > loose, "{prefix} {initials} {word} {inner} {loose}");
        assert_eq!(score("xyz", "Firefox"), None);
        assert_eq!(score("", "Firefox"), None);
    }

    #[test]
    fn shorter_names_win_ties_and_folding_works() {
        assert!(score("note", "Notepad").unwrap() > score("note", "Notepad++ Portable Edition").unwrap());
        assert!(score("cafe", "Café Manager").is_some());
        assert!(score("NOTE", "notepad").is_some());
        // camelCase word starts: "ps" → "PowerShell"
        assert!(score("ps", "PowerShell").unwrap() >= 800);
        assert!(score("vs code", "Visual Studio Code").is_some());
    }
}
