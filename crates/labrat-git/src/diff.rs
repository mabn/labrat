//! Tree diffs with rename detection and git-style unified patches.

use crate::Result;
use crate::object::{MODE_COMMIT, Oid, TreeEntry};
use crate::store::Repo;
use crate::xdiff::{self, Op};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct FileChange {
    pub old_path: String,
    pub new_path: String,
    /// Octal modes as strings ("100644"); "0" when the file is absent on that side.
    pub a_mode: String,
    pub b_mode: String,
    pub old_oid: Oid,
    pub new_oid: Oid,
    pub new_file: bool,
    pub deleted_file: bool,
    pub renamed_file: bool,
}

/// Flattens a tree into `path -> (mode, oid)` for blobs, links and submodules.
pub fn flatten(repo: &Repo, tree: &Oid) -> Result<BTreeMap<String, (u32, Oid)>> {
    let mut out = BTreeMap::new();
    flatten_into(repo, tree, "", &mut out)?;
    Ok(out)
}

fn flatten_into(
    repo: &Repo,
    tree: &Oid,
    prefix: &str,
    out: &mut BTreeMap<String, (u32, Oid)>,
) -> Result<()> {
    for e in repo.tree(tree)? {
        let path = if prefix.is_empty() {
            e.name.clone()
        } else {
            format!("{prefix}/{}", e.name)
        };
        if e.is_tree() {
            flatten_into(repo, &e.oid, &path, out)?;
        } else {
            out.insert(path, (e.mode, e.oid));
        }
    }
    Ok(())
}

fn mode_str(m: u32) -> String {
    format!("{m:o}")
}

/// Changes between two trees (`None` = empty tree), sorted by path, with renames detected.
pub fn diff_trees(repo: &Repo, old: Option<&Oid>, new: Option<&Oid>) -> Result<Vec<FileChange>> {
    let a = match old {
        Some(t) => flatten(repo, t)?,
        None => BTreeMap::new(),
    };
    let b = match new {
        Some(t) => flatten(repo, t)?,
        None => BTreeMap::new(),
    };
    let mut added: Vec<(String, u32, Oid)> = vec![];
    let mut deleted: Vec<(String, u32, Oid)> = vec![];
    let mut changes: Vec<FileChange> = vec![];
    for (path, (mode, oid)) in &a {
        match b.get(path) {
            Some((m2, o2)) if m2 == mode && o2 == oid => {}
            // A type change (file, symlink, submodule) is a deletion plus an addition.
            Some((m2, o2)) if (m2 & 0o170000) != (mode & 0o170000) => {
                changes.push(FileChange {
                    old_path: path.clone(),
                    new_path: path.clone(),
                    a_mode: mode_str(*mode),
                    b_mode: "0".into(),
                    old_oid: *oid,
                    new_oid: Oid::ZERO,
                    new_file: false,
                    deleted_file: true,
                    renamed_file: false,
                });
                changes.push(FileChange {
                    old_path: path.clone(),
                    new_path: path.clone(),
                    a_mode: "0".into(),
                    b_mode: mode_str(*m2),
                    old_oid: Oid::ZERO,
                    new_oid: *o2,
                    new_file: true,
                    deleted_file: false,
                    renamed_file: false,
                });
            }
            Some((m2, o2)) => changes.push(FileChange {
                old_path: path.clone(),
                new_path: path.clone(),
                a_mode: mode_str(*mode),
                b_mode: mode_str(*m2),
                old_oid: *oid,
                new_oid: *o2,
                new_file: false,
                deleted_file: false,
                renamed_file: false,
            }),
            None => deleted.push((path.clone(), *mode, *oid)),
        }
    }
    for (path, (mode, oid)) in &b {
        if !a.contains_key(path) {
            added.push((path.clone(), *mode, *oid));
        }
    }
    // Exact renames first, then similar content (git's default 50% threshold).
    let mut renames: Vec<(usize, usize)> = vec![];
    let mut used_add = vec![false; added.len()];
    let mut used_del = vec![false; deleted.len()];
    for (di, d) in deleted.iter().enumerate() {
        if let Some(ai) = added
            .iter()
            .enumerate()
            .position(|(ai, x)| !used_add[ai] && x.2 == d.2)
        {
            used_add[ai] = true;
            used_del[di] = true;
            renames.push((di, ai));
        }
    }
    if added.len() * deleted.len() <= 10_000 {
        let mut candidates: Vec<(u32, usize, usize)> = vec![];
        for (di, d) in deleted.iter().enumerate() {
            if used_del[di] || d.1 == MODE_COMMIT {
                continue;
            }
            let old = repo.blob(&d.2).unwrap_or_default();
            for (ai, x) in added.iter().enumerate() {
                if used_add[ai] || x.1 == MODE_COMMIT {
                    continue;
                }
                let new = repo.blob(&x.2).unwrap_or_default();
                let score = similarity(&old, &new);
                if score >= 50 {
                    candidates.push((score, di, ai));
                }
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, di, ai) in candidates {
            if !used_del[di] && !used_add[ai] {
                used_del[di] = true;
                used_add[ai] = true;
                renames.push((di, ai));
            }
        }
    }
    for (di, ai) in renames {
        let d = &deleted[di];
        let x = &added[ai];
        changes.push(FileChange {
            old_path: d.0.clone(),
            new_path: x.0.clone(),
            a_mode: mode_str(d.1),
            b_mode: mode_str(x.1),
            old_oid: d.2,
            new_oid: x.2,
            new_file: false,
            deleted_file: false,
            renamed_file: true,
        });
    }
    for (di, d) in deleted.iter().enumerate() {
        if !used_del[di] {
            changes.push(FileChange {
                old_path: d.0.clone(),
                new_path: d.0.clone(),
                a_mode: mode_str(d.1),
                b_mode: "0".into(),
                old_oid: d.2,
                new_oid: Oid::ZERO,
                new_file: false,
                deleted_file: true,
                renamed_file: false,
            });
        }
    }
    for (ai, x) in added.iter().enumerate() {
        if !used_add[ai] {
            changes.push(FileChange {
                old_path: x.0.clone(),
                new_path: x.0.clone(),
                a_mode: "0".into(),
                b_mode: mode_str(x.1),
                old_oid: Oid::ZERO,
                new_oid: x.2,
                new_file: true,
                deleted_file: false,
                renamed_file: false,
            });
        }
    }
    changes.sort_by(|a, b| {
        a.new_path
            .cmp(&b.new_path)
            .then(a.old_path.cmp(&b.old_path))
    });
    Ok(changes)
}

/// Rough content similarity in percent: bytes of common lines over the larger size.
fn similarity(a: &[u8], b: &[u8]) -> u32 {
    if a.is_empty() && b.is_empty() {
        return 100;
    }
    let la: Vec<&[u8]> = a.split_inclusive(|&c| c == b'\n').collect();
    let lb: Vec<&[u8]> = b.split_inclusive(|&c| c == b'\n').collect();
    let mut common = 0usize;
    for op in xdiff::ops(&xdiff::diff(&la, &lb), la.len(), lb.len()) {
        if let Op::Equal { old, len, .. } = op {
            common += la[old..old + len]
                .iter()
                .map(|l| l.len())
                .sum::<usize>();
        }
    }
    (common * 100 / a.len().max(b.len()).max(1)) as u32
}

/// Git's binary heuristic: a NUL byte in the first 8000 bytes.
pub fn is_binary(data: &[u8]) -> bool {
    data.iter().take(8000).any(|&b| b == 0)
}

fn lines(data: &[u8]) -> Vec<&[u8]> {
    data.split_inclusive(|&c| c == b'\n').collect()
}

/// Git's default funcname rule (`def_ff`): a line starting with an ASCII letter, `_` or `$`,
/// cut to 80 bytes and stripped of trailing whitespace.
fn funcname(line: &[u8]) -> Option<String> {
    let first = *line.first()?;
    if first.is_ascii_alphabetic() || first == b'_' || first == b'$' {
        let mut l = &line[..line.len().min(80)];
        while let Some((&c, rest)) = l.split_last() {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                l = rest;
            } else {
                break;
            }
        }
        Some(utf8_dropping_invalid(l))
    } else {
        None
    }
}

fn range(start: usize, len: usize) -> String {
    if len == 1 {
        format!("{}", start + 1)
    } else if len == 0 {
        format!("{start},0")
    } else {
        format!("{},{len}", start + 1)
    }
}

/// Unified patch between two blobs as GitLab returns it: starts at the first `@@` header,
/// 3 lines of context, git's funcname context and "\ No newline at end of file" markers.
pub fn patch(old: &[u8], new: &[u8]) -> String {
    const CONTEXT: usize = 3;
    let a = lines(old);
    let b = lines(new);
    let changes = xdiff::diff(&a, &b);
    let mut out = String::new();
    let mut i = 0;
    while i < changes.len() {
        // Changes at most 2 * CONTEXT lines apart share a hunk (`xdl_get_hunk`).
        let mut j = i + 1;
        while j < changes.len()
            && changes[j].old - (changes[j - 1].old + changes[j - 1].old_len) <= 2 * CONTEXT
        {
            j += 1;
        }
        let (first, last) = (changes[i], changes[j - 1]);
        let s1 = first.old.saturating_sub(CONTEXT);
        let s2 = first.new.saturating_sub(CONTEXT);
        let e1 = (last.old + last.old_len + CONTEXT).min(a.len());
        let e2 = (last.new + last.new_len + CONTEXT).min(b.len());
        out.push_str(&format!(
            "@@ -{} +{} @@",
            range(s1, e1 - s1),
            range(s2, e2 - s2)
        ));
        if let Some(f) = a[..s1].iter().rev().find_map(|l| funcname(l)) {
            out.push(' ');
            out.push_str(&f);
        }
        out.push('\n');
        let mut o = s1;
        for c in &changes[i..j] {
            for l in &a[o..c.old] {
                push_line(&mut out, ' ', l);
            }
            for l in &a[c.old..c.old + c.old_len] {
                push_line(&mut out, '-', l);
            }
            for l in &b[c.new..c.new + c.new_len] {
                push_line(&mut out, '+', l);
            }
            o = c.old + c.old_len;
        }
        for l in &a[o..e1] {
            push_line(&mut out, ' ', l);
        }
        i = j;
    }
    out
}

/// Decodes UTF-8, dropping invalid sequences (GitLab's diffs lose them rather than showing
/// replacement characters).
pub fn utf8_dropping_invalid(mut bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    loop {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let (ok, rest) = bytes.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(ok).unwrap_or_default());
                match e.error_len() {
                    Some(n) => bytes = &rest[n..],
                    None => return out,
                }
            }
        }
    }
}

thread_local! {
    /// Set while building a raw patch (`full_patch`): invalid UTF-8 is kept (as replacement
    /// characters) instead of dropped.
    static RAW_TEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn push_line(out: &mut String, prefix: char, line: &[u8]) {
    out.push(prefix);
    if RAW_TEXT.with(|r| r.get()) {
        out.push_str(&String::from_utf8_lossy(line));
    } else {
        out.push_str(&utf8_dropping_invalid(line));
    }
    if !line.ends_with(b"\n") {
        out.push_str("\n\\ No newline at end of file\n");
    }
}

/// The `diff` text GitLab shows for a change.
pub fn change_patch(repo: &Repo, c: &FileChange) -> Result<String> {
    let old = if c.old_oid.is_zero() || c.a_mode == "160000" {
        vec![]
    } else {
        repo.blob(&c.old_oid).unwrap_or_default()
    };
    let new = if c.new_oid.is_zero() || c.b_mode == "160000" {
        vec![]
    } else {
        repo.blob(&c.new_oid).unwrap_or_default()
    };
    if c.a_mode == "160000" || c.b_mode == "160000" {
        let o = if c.old_oid.is_zero() {
            String::new()
        } else {
            format!("Subproject commit {}\n", c.old_oid)
        };
        let n = if c.new_oid.is_zero() {
            String::new()
        } else {
            format!("Subproject commit {}\n", c.new_oid)
        };
        return Ok(patch(o.as_bytes(), n.as_bytes()));
    }
    if is_binary(&old) || is_binary(&new) {
        let a = if c.new_file { "/dev/null".to_string() } else { format!("a/{}", c.old_path) };
        let b = if c.deleted_file { "/dev/null".to_string() } else { format!("b/{}", c.new_path) };
        return Ok(format!("Binary files {a} and {b} differ\n"));
    }
    Ok(patch(&old, &new))
}

/// `Gitlab::Diff::FileCollectionSorter`: by path parts, directories before files at each level.
pub fn sort_like_gitlab(changes: &mut [FileChange]) {
    fn cmp(a: &[&str], b: &[&str]) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        let (ap, ar) = (a.first().copied().unwrap_or(""), a.get(1..).unwrap_or(&[]));
        let (bp, br) = (b.first().copied().unwrap_or(""), b.get(1..).unwrap_or(&[]));
        if ar.len() < br.len() && ar.is_empty() {
            return Greater;
        }
        if ar.len() > br.len() && br.is_empty() {
            return Less;
        }
        match ap.as_bytes().cmp(bp.as_bytes()) {
            Equal if !ar.is_empty() && !br.is_empty() => cmp(ar, br),
            o => o,
        }
    }
    changes.sort_by(|x, y| {
        let px = if x.new_path.is_empty() { &x.old_path } else { &x.new_path };
        let py = if y.new_path.is_empty() { &y.old_path } else { &y.new_path };
        let a: Vec<&str> = px.split('/').collect();
        let b: Vec<&str> = py.split('/').collect();
        cmp(&a, &b)
    });
}

/// Lines added and removed in a change (for stats).
pub fn line_stats(repo: &Repo, c: &FileChange) -> Result<(usize, usize)> {
    let old = if c.old_oid.is_zero() {
        vec![]
    } else {
        repo.blob(&c.old_oid).unwrap_or_default()
    };
    let new = if c.new_oid.is_zero() {
        vec![]
    } else {
        repo.blob(&c.new_oid).unwrap_or_default()
    };
    if is_binary(&old) || is_binary(&new) {
        return Ok((0, 0));
    }
    let a = lines(&old);
    let b = lines(&new);
    let (mut add, mut del) = (0, 0);
    for c in xdiff::diff(&a, &b) {
        add += c.new_len;
        del += c.old_len;
    }
    Ok((add, del))
}

pub fn entry_sort(entries: &mut [TreeEntry]) {
    entries.sort_by(|a, b| a.name.cmp(&b.name));
}

/// Git's `quote_c_style` (`core.quotePath` on): a name with control characters, `"`, `\\` or
/// non-ASCII bytes is wrapped in quotes with C escapes and octal bytes.
pub fn quote_path(name: &str) -> String {
    let needs = name.bytes().any(|b| b < 0x20 || b == 0x7f || b == b'"' || b == b'\\' || b >= 0x80);
    if !needs {
        return name.to_string();
    }
    let mut out = String::from("\"");
    for b in name.bytes() {
        match b {
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            0x0b => out.push_str("\\v"),
            0x0c => out.push_str("\\f"),
            b'\r' => out.push_str("\\r"),
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b if b < 0x20 || b >= 0x7f => out.push_str(&format!("\\{b:03o}")),
            b => out.push(b as char),
        }
    }
    out.push('"');
    out
}

/// Full `git diff` text (with `diff --git` headers) for a list of changes.
pub fn full_patch(repo: &Repo, changes: &[FileChange]) -> Result<String> {
    let prev = RAW_TEXT.with(|r| r.replace(true));
    let out = full_patch_inner(repo, changes);
    RAW_TEXT.with(|r| r.set(prev));
    out
}

fn full_patch_inner(repo: &Repo, changes: &[FileChange]) -> Result<String> {
    let mut out = String::new();
    for ch in changes {
        out.push_str(&format!(
            "diff --git {} {}\n",
            quote_path(&format!("a/{}", ch.old_path)),
            quote_path(&format!("b/{}", ch.new_path))
        ));
        let mut content_changed = ch.old_oid != ch.new_oid;
        if ch.new_file {
            out.push_str(&format!("new file mode {}\n", ch.b_mode));
        } else if ch.deleted_file {
            out.push_str(&format!("deleted file mode {}\n", ch.a_mode));
        } else if ch.a_mode != ch.b_mode {
            out.push_str(&format!("old mode {}\nnew mode {}\n", ch.a_mode, ch.b_mode));
        }
        if ch.renamed_file {
            let old = repo.blob(&ch.old_oid).unwrap_or_default();
            let new = repo.blob(&ch.new_oid).unwrap_or_default();
            out.push_str(&format!(
                "similarity index {}%\nrename from {}\nrename to {}\n",
                if ch.old_oid == ch.new_oid {
                    100
                } else {
                    similarity(&old, &new)
                },
                quote_path(&ch.old_path),
                quote_path(&ch.new_path)
            ));
            content_changed = ch.old_oid != ch.new_oid;
        }
        if !content_changed {
            continue;
        }
        let mode_suffix = if !ch.new_file && !ch.deleted_file && ch.a_mode == ch.b_mode {
            format!(" {}", ch.a_mode)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "index {}..{}{}\n",
            ch.old_oid, ch.new_oid, mode_suffix
        ));
        let text = change_patch(repo, ch)?;
        if text.starts_with("Binary files") {
            out.push_str(&text);
            continue;
        }
        // No hunks (an empty file created or deleted): git prints no ---/+++ lines.
        if text.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "--- {}\n",
            if ch.new_file {
                "/dev/null".into()
            } else {
                quote_path(&format!("a/{}", ch.old_path))
            }
        ));
        out.push_str(&format!(
            "+++ {}\n",
            if ch.deleted_file {
                "/dev/null".into()
            } else {
                quote_path(&format!("b/{}", ch.new_path))
            }
        ));
        out.push_str(&text);
    }
    Ok(out)
}

/// `git patch-id --stable` of a `git diff` text.
pub fn patch_id(diff: &str) -> Option<Oid> {
    use sha1::{Digest, Sha1};
    fn flush(result: &mut [u8; 20], ctx: &mut Sha1) {
        let hash: [u8; 20] = std::mem::take(ctx).finalize().into();
        let mut carry: u16 = 0;
        for i in 0..20 {
            carry += result[i] as u16 + hash[i] as u16;
            result[i] = carry as u8;
            carry >>= 8;
        }
    }
    fn scan_hunk(line: &str) -> (i64, i64) {
        // "@@ -a[,b] +c[,d] @@": counts default to 1.
        let mut parts = line.split_whitespace();
        parts.next();
        let count = |s: Option<&str>| -> i64 {
            s.and_then(|x| {
                x[1..]
                    .split_once(',')
                    .map(|(_, n)| n.parse().unwrap_or(1))
                    .or(Some(1))
            })
            .unwrap_or(1)
        };
        (count(parts.next()), count(parts.next()))
    }
    let mut result = [0u8; 20];
    let mut ctx = Sha1::new();
    let mut patchlen = 0usize;
    let (mut before, mut after): (i64, i64) = (-1, -1);
    let mut binary = false;
    let (mut pre, mut post) = (String::new(), String::new());
    for raw in diff.split_inclusive('\n') {
        let line = raw;
        if patchlen == 0 && !line.starts_with("diff ") {
            continue;
        }
        if before == -1 {
            if line.starts_with("GIT binary patch") || line.starts_with("Binary files") {
                binary = true;
                before = 0;
                ctx.update(pre.as_bytes());
                ctx.update(post.as_bytes());
                flush(&mut result, &mut ctx);
                continue;
            } else if let Some(rest) = line.strip_prefix("index ") {
                if let Some((a, b)) = rest.trim_end().split_once("..") {
                    pre = a.to_string();
                    post = b.split(' ').next().unwrap_or("").to_string();
                }
                continue;
            } else if line.starts_with("--- ") {
                before = 1;
                after = 1;
            } else if !line
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic())
            {
                break;
            }
        }
        if binary {
            if line.starts_with("diff ") {
                binary = false;
                before = -1;
            } else {
                continue;
            }
        }
        if before == 0 && after == 0 {
            if line.starts_with("@@ -") {
                let (b, a) = scan_hunk(line);
                before = b;
                after = a;
                continue;
            }
            if !line.starts_with("diff ") {
                break;
            }
            flush(&mut result, &mut ctx);
            before = -1;
            after = -1;
        }
        let first = line.as_bytes().first().copied().unwrap_or(0);
        if first == b'-' || first == b' ' {
            before -= 1;
        }
        if first == b'+' || first == b' ' {
            after -= 1;
        }
        let stripped: Vec<u8> = line.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        patchlen += stripped.len();
        ctx.update(&stripped);
    }
    if patchlen == 0 {
        return None;
    }
    flush(&mut result, &mut ctx);
    Some(Oid(result))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of a file diff with GitLab's running counters (`old_pos`/`new_pos`, 1-based), as
/// used for `line_code` (`<sha1(path)>_<old_pos>_<new_pos>`). Covers the whole file, not just
/// hunks, so unchanged lines outside hunks (GitLab's "unfolded" lines) are included.
#[derive(Debug, Clone, Copy)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_pos: usize,
    pub new_pos: usize,
}

impl DiffLine {
    /// `Line#old_line`: absent for added lines.
    pub fn old_line(&self) -> Option<usize> {
        if self.kind == LineKind::Added {
            None
        } else {
            Some(self.old_pos)
        }
    }

    /// `Line#new_line`: absent for removed lines.
    pub fn new_line(&self) -> Option<usize> {
        if self.kind == LineKind::Removed {
            None
        } else {
            Some(self.new_pos)
        }
    }
}

pub fn diff_lines(old: &[u8], new: &[u8]) -> Vec<DiffLine> {
    let a = lines(old);
    let b = lines(new);
    let mut out = vec![];
    // A missing side starts at 0, like the `@@ -0,0` / `+0,0` hunk headers GitLab parses.
    let (mut o, mut n) = (
        if old.is_empty() { 0usize } else { 1 },
        if new.is_empty() { 0usize } else { 1 },
    );
    for op in xdiff::ops(&xdiff::diff(&a, &b), a.len(), b.len()) {
        let (kind, len) = match op {
            Op::Equal { len, .. } => (LineKind::Context, len),
            Op::Delete { len, .. } => (LineKind::Removed, len),
            Op::Insert { len, .. } => (LineKind::Added, len),
        };
        for _ in 0..len {
            out.push(DiffLine {
                kind,
                old_pos: o,
                new_pos: n,
            });
            if kind != LineKind::Added {
                o += 1;
            }
            if kind != LineKind::Removed {
                n += 1;
            }
        }
    }
    out
}

/// `Gitlab::Git.diff_line_code`.
pub fn line_code(path: &str, old_pos: usize, new_pos: usize) -> String {
    use sha1::{Digest, Sha1};
    format!(
        "{}_{old_pos}_{new_pos}",
        hex::encode(Sha1::digest(path.as_bytes()))
    )
}

/// Maps line numbers of `old` to `new` for unchanged lines (1-based).
pub fn line_map(old: &[u8], new: &[u8]) -> std::collections::HashMap<usize, usize> {
    diff_lines(old, new)
        .into_iter()
        .filter(|l| l.kind == LineKind::Context)
        .map(|l| (l.old_pos, l.new_pos))
        .collect()
}
