//! Repository operations needed by project endpoints.

use labrat_core::model::{Project, User};
use labrat_core::repo::{self, FileAction, NewCommit};
use labrat_core::{Config, Result};
use rusqlite::Connection;

const README_TEMPLATE: &str = include_str!("../templates/readme_default.md");

/// `initialize_with_readme`: GitLab's default README as the first commit on the default branch.
pub fn initialize_with_readme(
    c: &Connection,
    config: &Config,
    p: &Project,
    author: &User,
    branch: &str,
) -> Result<()> {
    let content = README_TEMPLATE
        .replace("{{name}}", &p.name)
        .replace("{{description}}", p.description.as_deref().unwrap_or(""))
        .replace(
            "{{http_url}}",
            &config.url(&format!("/{}.git", p.full_path)),
        )
        .replace("{{default_branch}}", branch)
        .replace(
            "{{integrations_url}}",
            &config.url(&format!("/{}/-/settings/integrations", p.full_path)),
        );
    let oid = repo::create_commit(
        c,
        p,
        author,
        NewCommit {
            branch,
            start_branch: None,
            start_sha: None,
            message: "Initial commit",
            actions: &[FileAction::Create {
                path: "README.md".into(),
                content: content.into_bytes(),
                exec: None,
            }],
            author_name: None,
            author_email: None,
            force: false,
        },
    )?;
    // The README is a push like any other (post-receive: push event, hooks).
    let update = labrat_git::protocol::RefUpdate { old: labrat_git::Oid::ZERO, new: oid, name: format!("refs/heads/{branch}") };
    labrat_core::services::push::post_receive(c, config, p, author, &[update])?;
    Ok(())
}

pub fn branch_exists(c: &Connection, p: &Project, branch: &str) -> Result<bool> {
    Ok(repo::repo(c, p).branch(branch)?.is_some())
}
