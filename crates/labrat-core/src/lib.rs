//! labrat domain logic: storage, permissions and the services behind every user action.

pub mod chronic;
pub mod db;
pub mod error;
pub mod jobs;
pub mod model;
pub mod perms;
pub mod repo;
pub mod schema;
pub mod services;
pub mod time;

pub use db::Db;
pub use error::{Error, Result};

/// The GitLab version labrat reports and imitates.
pub const GITLAB_VERSION: &str = "18.11.14";
pub const GITLAB_REVISION: &str = "c2489ed0b24";
/// The revision GitLab EE 18.11.14 reports (`GET /version` on the EE reference instance).
pub const GITLAB_EE_REVISION: &str = "36cf8f5e8ce";

/// Which GitLab edition labrat imitates.
///
/// - `Ee`, the default, matches GitLab Enterprise Edition with a license that unlocks approval
///   rules.
/// - `Ce` matches the Community Edition exactly (`--noee`): EE-only routes and fields don't
///   exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edition {
    Ce,
    #[default]
    Ee,
}

impl Edition {
    pub fn is_ee(self) -> bool {
        self == Edition::Ee
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Base URL used in generated links, without a trailing slash.
    pub external_url: String,
    /// Host and port shown in `ssh_url_to_repo` (labrat has no SSH server).
    pub ssh_host: String,
    pub ssh_port: u16,
    /// Password of the `root` user created on an empty database.
    pub root_password: String,
    /// If set, `root` gets a personal access token with this exact value.
    pub root_token: Option<String>,
    /// The edition to imitate.
    pub edition: Edition,
}

impl Config {
    pub fn is_ee(&self) -> bool {
        self.edition.is_ee()
    }

    /// The version string GitLab reports (`/version`, `User-Agent`): EE adds `-ee`.
    pub fn gitlab_version(&self) -> String {
        if self.is_ee() { format!("{GITLAB_VERSION}-ee") } else { GITLAB_VERSION.to_string() }
    }

    pub fn gitlab_revision(&self) -> &'static str {
        if self.is_ee() { GITLAB_EE_REVISION } else { GITLAB_REVISION }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.external_url, path)
    }

    pub fn ssh_url(&self, full_path: &str) -> String {
        if self.ssh_port == 22 {
            format!("git@{}:{}.git", self.ssh_host, full_path)
        } else {
            format!(
                "ssh://git@{}:{}/{}.git",
                self.ssh_host, self.ssh_port, full_path
            )
        }
    }
}

/// Registers cross-module hooks (push processing for merge requests). Idempotent.
pub fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Branch pipelines come before the merge request refresh (and its MR pipelines).
        services::push::register_branch_hook(services::ci::on_push);
        services::push::register_branch_hook(services::merge_requests::refresh::on_push);
        services::push::register_branch_hook(services::hooks::on_push);
        services::push::register_branch_hook(services::cross_refs::on_push);
        services::push::register_mr_lookup(|c, p, branch| {
            Ok(c.query_row(
                "SELECT iid FROM merge_requests WHERE target_project_id = ?1 AND source_branch = ?2 AND state = 'opened' ORDER BY id LIMIT 1",
                rusqlite::params![p.id, branch],
                |r| r.get(0),
            )
            .ok())
        });
    });
}

pub struct App {
    pub db: Db,
    pub config: Config,
}
