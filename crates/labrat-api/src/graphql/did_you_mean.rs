//! Ruby's `DidYouMean::SpellChecker` (Jaro-Winkler, then Levenshtein), as graphql-ruby uses it for
//! " (Did you mean `x`?)" in validation messages (`ValidationContext#did_you_mean_suggestion`).

fn jaro(a: &[char], b: &[char]) -> f64 {
    let (s1, s2) = if a.len() > b.len() { (b, a) } else { (a, b) };
    let (l1, l2) = (s1.len(), s2.len());
    let range = (l2 / 2).saturating_sub(1);
    let mut flags1 = vec![false; l1];
    let mut flags2 = vec![false; l2 + range + 1];
    let mut m = 0.0f64;
    for i in 0..l1 {
        let last = i + range;
        let mut j = i.saturating_sub(range);
        if i < range {
            j = 0;
        }
        while j <= last {
            if !flags2[j] && j < l2 && s1[i] == s2[j] {
                flags2[j] = true;
                flags1[i] = true;
                m += 1.0;
                break;
            }
            j += 1;
        }
    }
    let mut t = 0.0f64;
    let mut k = 0usize;
    for i in 0..l1 {
        if flags1[i] {
            let mut j = k;
            let mut index = k;
            while j < l2 {
                index = j;
                if flags2[j] {
                    break;
                }
                j += 1;
            }
            k = j + 1;
            if index < l2 && s1[i] != s2[index] {
                t += 1.0;
            }
        }
    }
    let t = (t / 2.0).floor();
    if m == 0.0 { 0.0 } else { (m / l1 as f64 + m / l2 as f64 + (m - t) / m) / 3.0 }
}

fn jaro_winkler(a: &str, b: &str) -> f64 {
    let (ca, cb): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let d = jaro(&ca, &cb);
    if d > 0.7 {
        let mut prefix = 0;
        for (i, ch) in ca.iter().enumerate() {
            if i < 4 && cb.get(i) == Some(ch) {
                prefix += 1;
            } else {
                break;
            }
        }
        d + (prefix as f64 * 0.1 * (1.0 - d))
    } else {
        d
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut d: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = d[0];
        d[0] = i + 1;
        for j in 0..b.len() {
            let cur = d[j + 1];
            let cost = usize::from(*ca != b[j]);
            d[j + 1] = (cur + 1).min(d[j] + 1).min(prev + cost);
            prev = cur;
        }
    }
    d[b.len()]
}

fn normalize(s: &str) -> String {
    s.to_lowercase().replace('@', "")
}

/// `SpellChecker#correct`.
pub fn correct<'a>(input: &str, dictionary: &[&'a str]) -> Vec<&'a str> {
    let ni = normalize(input);
    let len = ni.chars().count();
    let threshold = if len > 3 { 0.834 } else { 0.77 };
    let mut words: Vec<&str> = dictionary.iter().copied().filter(|w| jaro_winkler(&normalize(w), &ni) >= threshold).filter(|w| *w != input).collect();
    words.sort_by(|a, b| jaro_winkler(b, &ni).partial_cmp(&jaro_winkler(a, &ni)).unwrap_or(std::cmp::Ordering::Equal));
    let t = (len as f64 * 0.25).ceil() as usize;
    let mut corrections: Vec<&str> = words.iter().copied().filter(|w| levenshtein(&normalize(w), &ni) <= t).collect();
    if corrections.is_empty() {
        corrections = words
            .iter()
            .copied()
            .filter(|w| {
                let w = normalize(w);
                levenshtein(&w, &ni) < len.min(w.chars().count())
            })
            .take(1)
            .collect();
    }
    corrections
}

/// `did_you_mean_suggestion`: "" or " (Did you mean `a`, `b` or `c`?)".
pub fn suggestion(input: &str, dictionary: &[&str]) -> String {
    let mut s = correct(input, dictionary);
    match s.len() {
        0 => String::new(),
        1 => format!(" (Did you mean `{}`?)", s[0]),
        _ => {
            let last = s.pop().unwrap();
            format!(" (Did you mean {} or `{last}`?)", s.iter().map(|x| format!("`{x}`")).collect::<Vec<_>>().join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_like_ruby() {
        assert_eq!(correct("Nope", &["Note", "Noteable", "Project"]), vec!["Note"]);
        assert!(correct("zzzzzz", &["Note", "Project"]).is_empty());
        assert_eq!(correct("titel", &["title", "description"]), vec!["title"]);
    }
}
