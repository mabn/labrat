//! Repository facts that project entities need.

use labrat_core::Result;
use labrat_core::model::Project;
use labrat_core::repo;
use rusqlite::Connection;

pub fn empty_repo(c: &Connection, p: &Project) -> Result<bool> {
    Ok(repo::repo(c, p).is_empty()?)
}

/// Path of the README on `branch`, if any (GitLab picks the first readme-like blob at the root).
pub fn readme_path(c: &Connection, p: &Project, branch: &str) -> Result<Option<String>> {
    let r = repo::repo(c, p);
    let Some(head) = r.branch(branch)? else {
        return Ok(None);
    };
    let tree = r.commit(&head)?.tree;
    let mut names: Vec<String> = r
        .tree(&tree)?
        .into_iter()
        .filter(|e| !e.is_tree() && e.name.to_lowercase().starts_with("readme"))
        .map(|e| e.name)
        .collect();
    // Prefer markup files, like GitLab's `Repository#readme`.
    names.sort_by_key(|n| {
        let l = n.to_lowercase();
        (
            if l.ends_with(".md")
                || l.ends_with(".markdown")
                || l.ends_with(".rst")
                || l.ends_with(".adoc")
            {
                0
            } else {
                1
            },
            n.clone(),
        )
    });
    Ok(names.into_iter().next())
}
