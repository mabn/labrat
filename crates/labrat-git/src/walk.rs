//! Commit graph walks: rev-list, ancestry, merge bases and reachable objects.

use crate::Result;
use crate::object::{Kind, Oid, Tag, parse_tree};
use crate::store::Repo;
use std::collections::{BinaryHeap, HashSet};

/// Commits reachable from `include` but not from `exclude`, newest first by committer
/// date (git's default `rev-list` order).
pub fn rev_list(repo: &Repo, include: &[Oid], exclude: &[Oid]) -> Result<Vec<Oid>> {
    rev_list_opts(repo, include, exclude, false)
}

pub fn rev_list_opts(
    repo: &Repo,
    include: &[Oid],
    exclude: &[Oid],
    first_parent: bool,
) -> Result<Vec<Oid>> {
    let hidden = ancestors(repo, exclude)?;
    let mut seen: HashSet<Oid> = HashSet::new();
    // (committer time, insertion sequence for stable ties, oid)
    let mut heap: BinaryHeap<(i64, i64, Oid)> = BinaryHeap::new();
    let mut seq = 0i64;
    for o in include {
        let o = repo.peel(o)?;
        if seen.insert(o) && !hidden.contains(&o) {
            let c = repo.commit(&o)?;
            heap.push((c.committer.time, -seq, o));
            seq += 1;
        }
    }
    let mut out = vec![];
    while let Some((_, _, oid)) = heap.pop() {
        out.push(oid);
        let c = repo.commit(&oid)?;
        let parents: Vec<Oid> = if first_parent {
            c.parents.iter().take(1).copied().collect()
        } else {
            c.parents.clone()
        };
        for p in parents {
            if !hidden.contains(&p) && seen.insert(p) {
                let pc = repo.commit(&p)?;
                heap.push((pc.committer.time, -seq, p));
                seq += 1;
            }
        }
    }
    Ok(out)
}

/// All commits reachable from `tips` (inclusive).
pub fn ancestors(repo: &Repo, tips: &[Oid]) -> Result<HashSet<Oid>> {
    let mut seen = HashSet::new();
    let mut stack: Vec<Oid> = vec![];
    for t in tips {
        if let Some(Kind::Commit) | Some(Kind::Tag) = repo.kind(t)? {
            stack.push(repo.peel(t)?);
        }
    }
    while let Some(o) = stack.pop() {
        if !seen.insert(o) {
            continue;
        }
        if let Ok(c) = repo.commit(&o) {
            stack.extend(c.parents);
        }
    }
    Ok(seen)
}

pub fn is_ancestor(repo: &Repo, ancestor: &Oid, descendant: &Oid) -> Result<bool> {
    Ok(ancestors(repo, &[*descendant])?.contains(ancestor))
}

/// Best common ancestor of `a` and `b` (git's `merge-base` for two commits).
pub fn merge_base(repo: &Repo, a: &Oid, b: &Oid) -> Result<Option<Oid>> {
    let anc_a = ancestors(repo, &[*a])?;
    let anc_b = ancestors(repo, &[*b])?;
    let common: HashSet<Oid> = anc_a.intersection(&anc_b).copied().collect();
    if common.is_empty() {
        return Ok(None);
    }
    // Keep only common ancestors that are not ancestors of another common ancestor.
    let mut best: Vec<Oid> = vec![];
    for c in &common {
        let mut dominated = false;
        for other in &common {
            if other != c && is_ancestor(repo, c, other)? {
                dominated = true;
                break;
            }
        }
        if !dominated {
            best.push(*c);
        }
    }
    best.sort_by_key(|o| std::cmp::Reverse(repo.commit(o).map(|c| c.committer.time).unwrap_or(0)));
    Ok(best.first().copied())
}

/// Every object reachable from `wants` but not from `haves`, commits first.
pub fn objects_for_pack(repo: &Repo, wants: &[Oid], haves: &[Oid]) -> Result<Vec<Oid>> {
    let mut have_objects: HashSet<Oid> = HashSet::new();
    let have_commits = ancestors(repo, haves)?;
    for c in &have_commits {
        have_objects.insert(*c);
        let commit = repo.commit(c)?;
        collect_tree(
            repo,
            &commit.tree,
            &mut have_objects,
            &HashSet::new(),
            &mut vec![],
        )?;
    }
    let mut out = vec![];
    let mut seen: HashSet<Oid> = HashSet::new();
    let mut commit_tips = vec![];
    for w in wants {
        match repo.kind(w)? {
            Some(Kind::Tag) => {
                let mut cur = *w;
                while let Some(Kind::Tag) = repo.kind(&cur)? {
                    if seen.insert(cur) && !have_objects.contains(&cur) {
                        out.push(cur);
                    }
                    cur = Tag::parse(&repo.read(&cur)?.1)?.object;
                }
                commit_tips.push(cur);
            }
            Some(Kind::Commit) => commit_tips.push(*w),
            Some(Kind::Tree) => collect_tree(repo, w, &mut seen, &have_objects, &mut out)?,
            Some(Kind::Blob) => {
                if seen.insert(*w) {
                    out.push(*w);
                }
            }
            None => {}
        }
    }
    let commits = rev_list(
        repo,
        &commit_tips,
        &have_commits.iter().copied().collect::<Vec<_>>(),
    )?;
    let mut trees = vec![];
    for c in &commits {
        if seen.insert(*c) {
            out.push(*c);
        }
        trees.push(repo.commit(c)?.tree);
    }
    for t in trees {
        collect_tree(repo, &t, &mut seen, &have_objects, &mut out)?;
    }
    Ok(out)
}

fn collect_tree(
    repo: &Repo,
    tree: &Oid,
    seen: &mut HashSet<Oid>,
    skip: &HashSet<Oid>,
    out: &mut Vec<Oid>,
) -> Result<()> {
    if skip.contains(tree) || !seen.insert(*tree) {
        return Ok(());
    }
    out.push(*tree);
    let (_, data) = repo.read(tree)?;
    for e in parse_tree(&data)? {
        if e.mode == crate::object::MODE_COMMIT {
            continue;
        }
        if e.is_tree() {
            collect_tree(repo, &e.oid, seen, skip, out)?;
        } else if !skip.contains(&e.oid) && seen.insert(e.oid) {
            out.push(e.oid);
        }
    }
    Ok(())
}

/// `deepen N` (upload-pack's `get_shallow_commits`): walking from `wants` (depth 1) or, when
/// `relative`, from the client's shallow commits (depth 0), the commits at depth `n` that have
/// parents become the new shallow boundary. Returns the boundary and the commits within reach.
pub fn shallow_boundary(repo: &Repo, wants: &[Oid], client_shallow: &[Oid], n: u64, relative: bool) -> Result<(HashSet<Oid>, HashSet<Oid>)> {
    let mut boundary = HashSet::new();
    let mut visited: HashSet<Oid> = HashSet::new();
    let (mut level, start_depth): (Vec<Oid>, u64) = if relative { (client_shallow.to_vec(), 0) } else { (wants.to_vec(), 1) };
    let mut depth = start_depth;
    while !level.is_empty() {
        let mut next = vec![];
        for c in level {
            if !visited.insert(c) {
                continue;
            }
            let Ok(commit) = repo.commit(&c) else { continue };
            if depth >= n {
                if !commit.parents.is_empty() {
                    boundary.insert(c);
                }
                continue;
            }
            next.extend(commit.parents.iter().copied());
        }
        level = next;
        depth += 1;
    }
    Ok((boundary, visited))
}

/// Objects for a shallow fetch: reachable from `wants` without crossing `boundary` commits'
/// parents, minus what the client has. The client's history ends at its own shallow commits,
/// so what it "has" stops there too.
pub fn objects_for_shallow_pack(repo: &Repo, wants: &[Oid], haves: &[Oid], boundary: &HashSet<Oid>, client_shallow: &HashSet<Oid>) -> Result<Vec<Oid>> {
    let walk = |starts: &[Oid], stop: &HashSet<Oid>| -> Result<Vec<Oid>> {
        let mut out = vec![];
        let mut seen = HashSet::new();
        let mut stack: Vec<Oid> = starts.to_vec();
        while let Some(c) = stack.pop() {
            if !seen.insert(c) {
                continue;
            }
            let Ok(commit) = repo.commit(&c) else { continue };
            out.push(c);
            if !stop.contains(&c) {
                stack.extend(commit.parents.iter().copied());
            }
        }
        Ok(out)
    };
    let had_commits: HashSet<Oid> = walk(haves, client_shallow)?.into_iter().collect();
    let mut have_objects: HashSet<Oid> = HashSet::new();
    for c in &had_commits {
        have_objects.insert(*c);
        collect_tree(repo, &repo.commit(c)?.tree, &mut have_objects, &HashSet::new(), &mut vec![])?;
    }
    let mut out = vec![];
    let mut seen: HashSet<Oid> = HashSet::new();
    let mut trees = vec![];
    for c in walk(wants, boundary)? {
        if had_commits.contains(&c) {
            continue;
        }
        if seen.insert(c) {
            out.push(c);
        }
        trees.push(repo.commit(&c)?.tree);
    }
    for t in trees {
        collect_tree(repo, &t, &mut seen, &have_objects, &mut out)?;
    }
    Ok(out)
}
