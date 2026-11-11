//! Three-way merges of trees and files, and the commit operations built on them.

use crate::diff::{flatten, is_binary};
use crate::object::{Commit, MODE_TREE, Oid, Signature, TreeEntry, encode_tree};
use crate::store::Repo;
use crate::{Kind, Result};
use crate::xdiff;
use std::collections::{BTreeMap, BTreeSet};

pub type FileMap = BTreeMap<String, (u32, Oid)>;

#[derive(Debug)]
pub struct MergeResult {
    pub tree: Oid,
    pub conflicts: Vec<String>,
}

impl MergeResult {
    pub fn clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Builds tree objects from a flat `path -> (mode, oid)` map.
pub fn write_tree(repo: &Repo, files: &FileMap) -> Result<Oid> {
    #[derive(Default)]
    struct Dir {
        files: Vec<TreeEntry>,
        dirs: BTreeMap<String, Dir>,
    }
    let mut root = Dir::default();
    for (path, (mode, oid)) in files {
        let mut parts: Vec<&str> = path.split('/').collect();
        let name = parts.pop().unwrap();
        let mut d = &mut root;
        for p in parts {
            d = d.dirs.entry(p.to_string()).or_default();
        }
        d.files.push(TreeEntry {
            mode: *mode,
            name: name.to_string(),
            oid: *oid,
        });
    }
    fn write(repo: &Repo, d: Dir) -> Result<Oid> {
        let mut entries = d.files;
        for (name, sub) in d.dirs {
            let oid = write(repo, sub)?;
            entries.push(TreeEntry {
                mode: MODE_TREE,
                name,
                oid,
            });
        }
        repo.write(Kind::Tree, &encode_tree(&mut entries))
    }
    write(repo, root)
}

/// Three-way merge of trees. Conflicting paths are listed; the result tree takes "ours" for them.
pub fn merge_trees(
    repo: &Repo,
    base: Option<&Oid>,
    ours: &Oid,
    theirs: &Oid,
) -> Result<MergeResult> {
    let b = match base {
        Some(t) => flatten(repo, t)?,
        None => BTreeMap::new(),
    };
    let mut o = flatten(repo, ours)?;
    let mut t = flatten(repo, theirs)?;
    // Renames (as merge-ort detects them, 50% similarity): when one side renamed a file the other
    // side modified in place, the modification follows the file to its new path.
    let mut b = b;
    if let Some(base_tree) = base {
        follow_renames(repo, base_tree, &mut b, &mut o, &mut t)?;
        follow_renames(repo, base_tree, &mut b, &mut t, &mut o)?;
    }
    let (o, t) = (o, t);
    let mut paths: BTreeSet<&String> = BTreeSet::new();
    paths.extend(b.keys());
    paths.extend(o.keys());
    paths.extend(t.keys());
    let mut out: FileMap = BTreeMap::new();
    let mut conflicts = vec![];
    for path in paths {
        let (eb, eo, et) = (b.get(path), o.get(path), t.get(path));
        let chosen = if eo == et {
            eo.copied()
        } else if eb == eo {
            et.copied()
        } else if eb == et {
            eo.copied()
        } else {
            match (eb, eo, et) {
                (_, Some(oe), Some(te))
                    if oe.0 != crate::object::MODE_COMMIT && te.0 != crate::object::MODE_COMMIT =>
                {
                    let base_data = match eb {
                        Some(be) => repo.blob(&be.1)?,
                        None => vec![],
                    };
                    let od = repo.blob(&oe.1)?;
                    let td = repo.blob(&te.1)?;
                    match merge_files(&base_data, &od, &td) {
                        Some(merged) => {
                            let mode = if eb.map(|x| x.0) == Some(oe.0) {
                                te.0
                            } else {
                                oe.0
                            };
                            Some((mode, repo.write(Kind::Blob, &merged)?))
                        }
                        None => {
                            conflicts.push(path.clone());
                            eo.copied()
                        }
                    }
                }
                _ => {
                    conflicts.push(path.clone());
                    eo.copied().or(et.copied())
                }
            }
        };
        if let Some(e) = chosen {
            out.insert(path.clone(), e);
        }
    }
    // A path that is a file on one side and a directory on the other is a conflict.
    let keys: Vec<String> = out.keys().cloned().collect();
    for k in &keys {
        let prefix = format!("{k}/");
        if keys.iter().any(|x| x.starts_with(&prefix)) && !conflicts.contains(k) {
            conflicts.push(k.clone());
            out.remove(k);
        }
    }
    conflicts.sort();
    Ok(MergeResult {
        tree: write_tree(repo, &out)?,
        conflicts,
    })
}

/// Applies `other`'s in-place edits of files that `renamer` renamed to the new paths: both sides
/// then hold the merged content at the new path and nothing at the old one.
fn follow_renames(repo: &Repo, base_tree: &Oid, b: &mut FileMap, renamer: &mut FileMap, other: &mut FileMap) -> Result<()> {
    let side_tree = write_tree(repo, renamer)?;
    for ch in crate::diff::diff_trees(repo, Some(base_tree), Some(&side_tree))? {
        if !ch.renamed_file {
            continue;
        }
        let (from, to) = (ch.old_path.clone(), ch.new_path.clone());
        let (Some(be), Some(oe)) = (b.get(&from).copied(), other.get(&from).copied()) else { continue };
        if oe == be || other.contains_key(&to) {
            continue;
        }
        let Some(re) = renamer.get(&to).copied() else { continue };
        if let Some(data) = merge_files(&repo.blob(&be.1)?, &repo.blob(&re.1)?, &repo.blob(&oe.1)?) {
            let e = (re.0, repo.write(Kind::Blob, &data)?);
            renamer.insert(to.clone(), e);
            other.insert(to, e);
            other.remove(&from);
            b.remove(&from);
        }
    }
    Ok(())
}

struct Change<'a> {
    start: usize,
    end: usize,
    lines: Vec<&'a [u8]>,
    side: u8,
}

fn changes<'a>(base: &[&'a [u8]], side: &[&'a [u8]], tag: u8) -> Vec<Change<'a>> {
    let mut out: Vec<Change> = vec![];
    // git's merge machinery diffs without the indent heuristic.
    for c in xdiff::diff_with(base, side, false) {
        let (start, end, new_range) = (c.old, c.old + c.old_len, c.new..c.new + c.new_len);
        let lines: Vec<&[u8]> = side[new_range].to_vec();
        match out.last_mut() {
            Some(prev) if prev.end == start && prev.side == tag => {
                prev.end = end;
                prev.lines.extend(lines);
            }
            _ => out.push(Change {
                start,
                end,
                lines,
                side: tag,
            }),
        }
    }
    out
}

/// Line-based three-way merge. Returns `None` on conflict; overlapping or adjacent changes
/// from both sides conflict unless they are identical, as in git.
pub fn merge_files(base: &[u8], ours: &[u8], theirs: &[u8]) -> Option<Vec<u8>> {
    if ours == theirs {
        return Some(ours.to_vec());
    }
    if base == ours {
        return Some(theirs.to_vec());
    }
    if base == theirs {
        return Some(ours.to_vec());
    }
    if is_binary(base) || is_binary(ours) || is_binary(theirs) {
        return None;
    }
    let bl: Vec<&[u8]> = base.split_inclusive(|&c| c == b'\n').collect();
    let ol: Vec<&[u8]> = ours.split_inclusive(|&c| c == b'\n').collect();
    let tl: Vec<&[u8]> = theirs.split_inclusive(|&c| c == b'\n').collect();
    let mut all = changes(&bl, &ol, 0);
    all.extend(changes(&bl, &tl, 1));
    all.sort_by(|a, b| (a.start, a.end, a.side).cmp(&(b.start, b.end, b.side)));
    let mut out: Vec<u8> = vec![];
    let mut pos = 0;
    let mut i = 0;
    while i < all.len() {
        let mut group_end = all[i].end;
        let mut j = i + 1;
        while j < all.len() && all[j].start <= group_end {
            group_end = group_end.max(all[j].end);
            j += 1;
        }
        let group = &all[i..j];
        let gs = group[0].start;
        for l in &bl[pos..gs] {
            out.extend_from_slice(l);
        }
        let render = |side: u8| -> Vec<u8> {
            let mut v = vec![];
            let mut p = gs;
            for c in group.iter().filter(|c| c.side == side) {
                for l in &bl[p..c.start] {
                    v.extend_from_slice(l);
                }
                for l in &c.lines {
                    v.extend_from_slice(l);
                }
                p = c.end;
            }
            for l in &bl[p..group_end] {
                v.extend_from_slice(l);
            }
            v
        };
        let sides: BTreeSet<u8> = group.iter().map(|c| c.side).collect();
        if sides.len() == 1 {
            out.extend(render(*sides.iter().next().unwrap()));
        } else {
            let a = render(0);
            let b = render(1);
            if a != b {
                return None;
            }
            out.extend(a);
        }
        pos = group_end;
        i = j;
    }
    for l in &bl[pos..] {
        out.extend_from_slice(l);
    }
    Some(out)
}

/// Replays `commits` (oldest first) on top of `onto`, like `git rebase`. Returns the new head
/// or the path list of the first conflict.
pub fn rebase(
    repo: &Repo,
    onto: &Oid,
    commits: &[Oid],
    committer: &Signature,
) -> Result<std::result::Result<Oid, Vec<String>>> {
    let mut head = *onto;
    for c in commits {
        let commit = repo.commit(c)?;
        let parent_tree = match commit.parents.first() {
            Some(p) => Some(repo.commit(p)?.tree),
            None => None,
        };
        let head_tree = repo.commit(&head)?.tree;
        let result = merge_trees(repo, parent_tree.as_ref(), &head_tree, &commit.tree)?;
        if !result.clean() {
            return Ok(Err(result.conflicts));
        }
        if result.tree == head_tree && parent_tree.as_ref() != Some(&commit.tree) {
            // The change is already present upstream; git drops such commits.
            continue;
        }
        let new = Commit {
            tree: result.tree,
            parents: vec![head],
            author: commit.author.clone(),
            committer: committer.clone(),
            extra: vec![],
            message: commit.message.clone(),
        };
        head = repo.write_commit(&new)?;
    }
    Ok(Ok(head))
}
