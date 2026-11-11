//! Differential test: labrat's unified patches against `git diff --no-index -U3` on generated
//! file pairs (random edits over small alphabets so ties are frequent, indented code with
//! inserted functions, blank lines, moved and duplicated blocks, missing trailing newlines,
//! CRLF, long files that trip xdiff's cost heuristics).
//!
//! `cargo test -p labrat-git --test xdiff_vs_git` runs a fast subset; the `#[ignore]`d test runs
//! a large one (`XDIFF_CASES` / `XDIFF_SEED` override its size and seed). Skipped when no
//! `git` binary is available.

use labrat_git::diff::patch;
use std::path::Path;
use std::process::Command;

struct Rng(std::cell::Cell<u64>);

impl Rng {
    fn next(&self) -> u64 {
        // xorshift64*
        let mut x = self.0.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0.set(x);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&self, pct: usize) -> bool {
        self.below(100) < pct
    }
    fn pick<'a>(&self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
}

const SMALL: &[&str] = &["a", "b", "c", "d", "", "}", "    x", "\ty"];

fn random_lines(rng: &Rng, alphabet: &[&str], n: usize) -> Vec<String> {
    (0..n).map(|_| rng.pick(alphabet).to_string()).collect()
}

fn word_alphabet(rng: &Rng, size: usize) -> Vec<String> {
    (0..size)
        .map(|i| {
            let indent = ["", "  ", "    ", "\t", "        "][rng.below(5)];
            if rng.chance(10) {
                String::new()
            } else {
                format!("{indent}w{i}")
            }
        })
        .collect()
}

/// A random function in a C-like or Python-like style.
fn function(rng: &Rng, name: &str, python: bool) -> Vec<String> {
    let mut out = vec![];
    let body = 1 + rng.below(6);
    if python {
        out.push(format!("def {name}(x):"));
        for _ in 0..body {
            match rng.below(4) {
                0 => {
                    out.push("    if x:".into());
                    out.push("        return x".into());
                }
                1 => out.push(String::new()),
                _ => out.push(format!("    x = x + {}", rng.below(3))),
            }
        }
        out.push("    return x".into());
    } else {
        out.push(format!("int {name}(int x)"));
        out.push("{".into());
        for _ in 0..body {
            match rng.below(4) {
                0 => {
                    out.push("\tif (x) {".into());
                    out.push("\t\treturn x;".into());
                    out.push("\t}".into());
                }
                1 => out.push(String::new()),
                _ => out.push(format!("\tx += {};", rng.below(3))),
            }
        }
        out.push("\treturn x;".into());
        out.push("}".into());
    }
    out
}

fn code_file(rng: &Rng) -> (Vec<String>, Vec<String>) {
    let python = rng.chance(50);
    let n = 2 + rng.below(6);
    let mut funcs: Vec<Vec<String>> = (0..n)
        .map(|i| function(rng, &format!("f{}", i % 3), python))
        .collect();
    let join = |fs: &[Vec<String>], blanks: usize| {
        let mut v = vec![];
        for (i, f) in fs.iter().enumerate() {
            if i > 0 {
                v.extend(std::iter::repeat_n(String::new(), blanks));
            }
            v.extend(f.iter().cloned());
        }
        v
    };
    let blanks = rng.below(3);
    let old = join(&funcs, blanks);
    match rng.below(3) {
        0 => {
            let at = rng.below(funcs.len() + 1);
            let f = function(rng, &format!("f{}", rng.below(3)), python);
            funcs.insert(at, f);
        }
        1 => {
            let at = rng.below(funcs.len());
            funcs.remove(at);
        }
        _ => {
            let at = rng.below(funcs.len());
            let f = funcs[at].clone();
            funcs.insert(rng.below(funcs.len() + 1), f);
        }
    }
    let new = join(&funcs, blanks);
    (old, edit(rng, &new, rng.below(2)))
}

/// Applies `n` random block edits (insert, delete, replace, move, duplicate).
fn edit(rng: &Rng, lines: &[String], n: usize) -> Vec<String> {
    let mut v = lines.to_vec();
    for _ in 0..n {
        let len = v.len();
        let at = rng.below(len + 1);
        let size = 1 + rng.below(if rng.chance(20) { 12 } else { 4 });
        match rng.below(5) {
            0 => {
                let block: Vec<String> = if len > 0 && rng.chance(50) {
                    let s = rng.below(len);
                    v[s..(s + size).min(len)].to_vec()
                } else {
                    random_lines(rng, SMALL, size)
                };
                v.splice(at..at, block);
            }
            1 => {
                let end = (at + size).min(len);
                v.drain(at..end);
            }
            2 => {
                let end = (at + size).min(len);
                let block = random_lines(rng, SMALL, 1 + rng.below(4));
                v.splice(at..end, block);
            }
            3 => {
                let end = (at + size).min(len);
                let block: Vec<String> = v.drain(at..end).collect();
                let to = rng.below(v.len() + 1);
                v.splice(to..to, block);
            }
            _ => {
                if len > 0 {
                    let s = rng.below(len);
                    let block = v[s..(s + size).min(len)].to_vec();
                    v.splice(at..at, block);
                }
            }
        }
    }
    v
}

fn render(rng: &Rng, lines: &[String], crlf: usize, no_eol: bool) -> Vec<u8> {
    let mut out = vec![];
    for l in lines {
        out.extend_from_slice(l.as_bytes());
        if crlf > 0 && rng.chance(crlf) {
            out.push(b'\r');
        }
        out.push(b'\n');
    }
    if no_eol && out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

fn case(rng: &Rng, long: bool) -> (Vec<u8>, Vec<u8>) {
    let kind = rng.below(if long { 10 } else { 6 });
    let (old, new) = match kind {
        0 | 1 => {
            let alpha: Vec<&str> = SMALL[..2 + rng.below(SMALL.len() - 1)].to_vec();
            let old = random_lines(rng, &alpha, rng.below(25));
            let new = if rng.chance(30) {
                random_lines(rng, &alpha, rng.below(25))
            } else {
                edit(rng, &old, 1 + rng.below(4))
            };
            (old, new)
        }
        2 | 3 => code_file(rng),
        4 => {
            let words = word_alphabet(rng, 3 + rng.below(30));
            let alpha: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
            let old = random_lines(rng, &alpha, rng.below(200));
            let new = edit(rng, &old, 1 + rng.below(10));
            (old, new)
        }
        5 => {
            // Moved blocks of distinct lines.
            let n = 3 + rng.below(30);
            let old: Vec<String> = (0..n).map(|i| format!("line {i}")).collect();
            let mut new = old.clone();
            let s = rng.below(n);
            let e = (s + 1 + rng.below(n - s)).min(n);
            let block: Vec<String> = new.drain(s..e).collect();
            let to = rng.below(new.len() + 1);
            new.splice(to..to, block);
            (old, edit(rng, &new, rng.below(2)))
        }
        6 => {
            // Long files with many differences: exercises the cost heuristics.
            let words = word_alphabet(rng, 20 + rng.below(400));
            let alpha: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
            let old = random_lines(rng, &alpha, 1000 + rng.below(4000));
            let new = if rng.chance(30) {
                random_lines(rng, &alpha, 1000 + rng.below(4000))
            } else {
                edit(rng, &old, 50 + rng.below(400))
            };
            (old, new)
        }
        8 => {
            // Long files of mostly unique lines with many spaced-out edits: long snakes
            // between changes and a high edit cost (xdl_split's snake heuristics).
            // Above ~33k non-discarded lines per side xdl_split's cost limit exceeds XDL_HEUR_MIN_COST.
            let n = if rng.chance(50) { 2000 + rng.below(6000) } else { 40000 + rng.below(6000) };
            let old: Vec<String> = (0..n).map(|i| format!("L{i}")).collect();
            let mut new = vec![];
            let mut i = 0;
            let mut tag = 0;
            while i < n {
                let keep = 15 + rng.below(20);
                new.extend(old[i..(i + keep).min(n)].iter().cloned());
                i += keep;
                i += rng.below(4);
                for _ in 0..rng.below(4) {
                    // Mostly copies of other lines: unique new lines would be discarded
                    // before the search and never cost anything.
                    if rng.chance(80) {
                        new.push(old[rng.below(n)].clone());
                    } else {
                        tag += 1;
                        new.push(format!("N{tag}"));
                    }
                }
            }
            (old, new)
        }
        9 => {
            // Frequent lines ("", braces) around long unmatched runs (xdl_clean_mmatch).
            let common = ["", "}", "{", "\t}", "  end"];
            let line = |prefix: &str, i: usize| {
                if rng.chance(40) {
                    common[rng.below(common.len())].to_string()
                } else {
                    format!("{prefix}{i}")
                }
            };
            let mut old = vec![];
            let mut new = vec![];
            for b in 0..10 + rng.below(40) {
                let len = 5 + rng.below(if rng.chance(30) { 300 } else { 30 });
                if rng.chance(50) {
                    for i in 0..len {
                        let l = line("s", b * 1000 + i);
                        old.push(l.clone());
                        new.push(l);
                    }
                } else {
                    for i in 0..len {
                        old.push(line("o", b * 1000 + i));
                    }
                    for i in 0..len + rng.below(20) {
                        new.push(line("n", b * 1000 + i));
                    }
                }
            }
            (old, new)
        }
        _ => {
            // Long code with a few edits.
            let mut old = vec![];
            let mut new = vec![];
            for _ in 0..20 + rng.below(80) {
                let (o, n) = code_file(rng);
                old.extend(o);
                new.extend(n);
            }
            (old, new)
        }
    };
    let crlf = if rng.chance(10) { [10, 100][rng.below(2)] } else { 0 };
    let a = render(rng, &old, crlf, rng.chance(15));
    let b = render(rng, &new, crlf, rng.chance(15));
    (a, b)
}

/// Runs git over all cases at once (two directories) and returns each file's patch body,
/// starting at its first `@@` line.
fn git_patches(dir: &Path, n: usize) -> Option<Vec<String>> {
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", dir)
        .args([
            "diff",
            "--no-index",
            "--no-color",
            "--no-renames",
            "--no-ext-diff",
            "-U3",
            "a",
            "b",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).expect("ASCII output");
    let mut patches = vec![String::new(); n];
    let mut current: Option<usize> = None;
    let mut in_body = false;
    for line in text.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("diff --git a/a/") {
            let name = rest.split(' ').next().unwrap();
            current = Some(name.trim_start_matches("case").parse().unwrap());
            in_body = false;
            continue;
        }
        if line.starts_with("@@") {
            in_body = true;
        }
        if in_body {
            patches[current.unwrap()].push_str(line);
        }
    }
    Some(patches)
}

fn run(seed: u64, n: usize, long_every: usize) {
    let tmp = std::env::temp_dir().join(format!("labrat-xdiff-{}-{seed}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("a")).unwrap();
    std::fs::create_dir_all(tmp.join("b")).unwrap();
    let rng = Rng(std::cell::Cell::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1));
    let mut cases = vec![];
    for i in 0..n {
        let (a, b) = case(&rng, long_every > 0 && i % long_every == 0);
        std::fs::write(tmp.join("a").join(format!("case{i}")), &a).unwrap();
        std::fs::write(tmp.join("b").join(format!("case{i}")), &b).unwrap();
        cases.push((a, b));
    }
    let Some(expected) = git_patches(&tmp, n) else {
        eprintln!("git not available; skipping");
        return;
    };
    let nonempty = expected.iter().filter(|p| !p.is_empty()).count();
    assert!(nonempty * 10 >= n * 8, "only {nonempty} of {n} git patches are non-empty");
    let mut failures = vec![];
    for (i, (a, b)) in cases.iter().enumerate() {
        let got = patch(a, b);
        if got != expected[i] {
            failures.push(i);
        }
    }
    if failures.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp);
    } else {
        let i = failures[0];
        panic!(
            "{} of {n} cases differ from git (seed {seed}); first: case{i} in {}\n--- git\n{}\n--- labrat\n{}",
            failures.len(),
            tmp.display(),
            expected[i],
            patch(&cases[i].0, &cases[i].1)
        );
    }
}

#[test]
fn matches_git_fast() {
    run(1, 600, 100);
}

#[test]
#[ignore]
fn matches_git_large() {
    let n = std::env::var("XDIFF_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let seed = std::env::var("XDIFF_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(42);
    run(seed, n, 25);
}

/// `merge_files` against `git merge-file -p` (xdiff's three-way merge) on small random edits
/// of the same base: same clean result, or a conflict on both sides.
#[test]
fn merge_matches_git_merge_file() {
    let dir = std::env::temp_dir().join(format!("labrat-xmerge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rng = Rng(std::cell::Cell::new(0x1234_5678_9abc_def1));
    let alpha = ["a", "b", "c", "", "}"];
    let join = |v: &[String]| {
        let mut s = v.join("\n");
        s.push('\n');
        s.into_bytes()
    };
    for i in 0..400 {
        let base = random_lines(&rng, &alpha, rng.below(15));
        let ours = edit(&rng, &base, 1 + rng.below(3));
        let theirs = edit(&rng, &base, 1 + rng.below(3));
        let (b, o, t) = (join(&base), join(&ours), join(&theirs));
        std::fs::write(dir.join("base"), &b).unwrap();
        std::fs::write(dir.join("ours"), &o).unwrap();
        std::fs::write(dir.join("theirs"), &t).unwrap();
        let Ok(out) = Command::new("git")
            .current_dir(&dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["merge-file", "-p", "ours", "base", "theirs"])
            .output()
        else {
            eprintln!("git not available; skipping");
            return;
        };
        let git = (out.status.code() == Some(0)).then_some(out.stdout);
        let ours_merge = labrat_git::merge::merge_files(&b, &o, &t);
        assert_eq!(
            git.as_deref().map(String::from_utf8_lossy),
            ours_merge.as_deref().map(String::from_utf8_lossy),
            "case {i}: base {base:?} ours {ours:?} theirs {theirs:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn known_ties() {
    assert_eq!(
        patch(
            b"one\ntwo\nthree\nfour\nfive\nsix\n",
            b"four\nfive\nsix\none\ntwo\nthree\n"
        ),
        "@@ -1,6 +1,6 @@\n-one\n-two\n-three\n four\n five\n six\n+one\n+two\n+three\n"
    );
    // Indent heuristic: the inserted function is shown whole, not shifted by the shared
    // closing lines.
    let old = b"def a():\n    return 1\n\ndef c():\n    return 3\n";
    let new = b"def a():\n    return 1\n\ndef b():\n    return 1\n\ndef c():\n    return 3\n";
    assert_eq!(
        patch(old, new),
        "@@ -1,5 +1,8 @@\n def a():\n     return 1\n \n+def b():\n+    return 1\n+\n def c():\n     return 3\n"
    );
}
