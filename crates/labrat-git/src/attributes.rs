//! `.gitattributes`: patterns (git's wildmatch with pathname semantics) and attribute lookup.

/// One attribute setting on a line: `name` (set), `-name` (unset), `name=value`.
#[derive(Debug, Clone, PartialEq)]
pub enum Setting {
    Set,
    Unset,
    Value(String),
}

#[derive(Debug, Clone)]
struct Line {
    pattern: String,
    attrs: Vec<(String, Setting)>,
}

/// The parsed root `.gitattributes` file.
#[derive(Debug, Clone, Default)]
pub struct Attributes {
    lines: Vec<Line>,
}

impl Attributes {
    pub fn parse(text: &str) -> Attributes {
        let mut lines = vec![];
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(pattern) = parts.next() else { continue };
            let attrs = parts
                .map(|a| {
                    if let Some(n) = a.strip_prefix('-') {
                        (n.to_string(), Setting::Unset)
                    } else if let Some(n) = a.strip_prefix('!') {
                        // `!name` returns the attribute to unspecified; treat it as unset here.
                        (n.to_string(), Setting::Unset)
                    } else if let Some((n, v)) = a.split_once('=') {
                        (n.to_string(), Setting::Value(v.to_string()))
                    } else {
                        (a.to_string(), Setting::Set)
                    }
                })
                .collect();
            lines.push(Line { pattern: pattern.to_string(), attrs });
        }
        Attributes { lines }
    }

    /// The effective setting of `attr` for `path`: the last matching line wins.
    pub fn get(&self, path: &str, attr: &str) -> Option<&Setting> {
        let mut found = None;
        for l in &self.lines {
            if pattern_matches(&l.pattern, path)
                && let Some((_, s)) = l.attrs.iter().rev().find(|(n, _)| n == attr)
            {
                found = Some(s);
            }
        }
        found
    }
}

/// gitattributes pattern semantics: a pattern without a slash matches the basename at any level;
/// otherwise it is matched against the whole path from the root (a leading `/` is dropped).
pub fn pattern_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.strip_prefix('/').map(|p| (p, true)).unwrap_or((pattern, false));
    let (pat, anchored) = pattern;
    if !anchored && !pat.contains('/') {
        let base = path.rsplit('/').next().unwrap_or(path);
        return wildmatch(pat.as_bytes(), base.as_bytes());
    }
    wildmatch(pat.as_bytes(), path.as_bytes())
}

/// Git's `wildmatch` with `WM_PATHNAME`: `*` and `?` don't cross `/`, `**/` matches any number of
/// directories, a trailing `/**` everything inside, and `[...]` character classes.
pub fn wildmatch(p: &[u8], t: &[u8]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    // `**` as a whole path component.
    if p.starts_with(b"**") {
        let rest = &p[2..];
        if rest.is_empty() {
            return true;
        }
        if rest[0] == b'/' {
            let rest = &rest[1..];
            // Zero directories, or skip one component at a time.
            if wildmatch(rest, t) {
                return true;
            }
            let mut i = 0;
            while i < t.len() {
                if t[i] == b'/' && wildmatch(rest, &t[i + 1..]) {
                    return true;
                }
                i += 1;
            }
            return false;
        }
    }
    match p[0] {
        b'*' => {
            let rest = &p[1..];
            let mut i = 0;
            loop {
                if wildmatch(rest, &t[i..]) {
                    return true;
                }
                if i >= t.len() || t[i] == b'/' {
                    return false;
                }
                i += 1;
            }
        }
        b'?' => !t.is_empty() && t[0] != b'/' && wildmatch(&p[1..], &t[1..]),
        b'[' => {
            if t.is_empty() || t[0] == b'/' {
                return false;
            }
            let Some(end) = p[1..].iter().position(|&c| c == b']').map(|e| e + 1) else {
                return t[0] == b'[' && wildmatch(&p[1..], &t[1..]);
            };
            let class = &p[1..end];
            let (negate, class) = match class.first() {
                Some(b'!') | Some(b'^') => (true, &class[1..]),
                _ => (false, class),
            };
            let c = t[0];
            let mut hit = false;
            let mut i = 0;
            while i < class.len() {
                if i + 2 < class.len() && class[i + 1] == b'-' {
                    if class[i] <= c && c <= class[i + 2] {
                        hit = true;
                    }
                    i += 3;
                } else {
                    if class[i] == c {
                        hit = true;
                    }
                    i += 1;
                }
            }
            hit != negate && wildmatch(&p[end + 1..], &t[1..])
        }
        b'\\' if p.len() > 1 => !t.is_empty() && t[0] == p[1] && wildmatch(&p[2..], &t[1..]),
        c => !t.is_empty() && t[0] == c && wildmatch(&p[1..], &t[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(pattern_matches("*.lock", "a/b/yarn.lock"));
        assert!(pattern_matches("gen/attr.txt", "gen/attr.txt"));
        assert!(!pattern_matches("gen/attr.txt", "x/gen/attr.txt"));
        assert!(pattern_matches("/top.txt", "top.txt"));
        assert!(!pattern_matches("/top.txt", "a/top.txt"));
        assert!(pattern_matches("docs/**", "docs/a/b.md"));
        assert!(pattern_matches("**/vendor/*.js", "a/vendor/x.js"));
        assert!(pattern_matches("**/vendor/*.js", "vendor/x.js"));
        assert!(!pattern_matches("src/*.rs", "src/a/b.rs"));
        assert!(pattern_matches("file[0-9].txt", "file7.txt"));
    }

    #[test]
    fn last_line_wins() {
        let a = Attributes::parse("*.js linguist-generated\nkeep.js -linguist-generated\n");
        assert_eq!(a.get("x.js", "linguist-generated"), Some(&Setting::Set));
        assert_eq!(a.get("keep.js", "linguist-generated"), Some(&Setting::Unset));
        assert_eq!(a.get("x.rs", "linguist-generated"), None);
    }
}
