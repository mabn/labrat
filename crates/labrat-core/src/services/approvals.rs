//! EE approval rules and approval settings (GitLab Premium and Ultimate).
//!
//! Everything here applies only when labrat runs as EE (`Config::is_ee`). On CE the routes don't
//! exist and the MR approvals body is CE's.

use crate::Result;
use crate::model::Project;
use crate::services::merge_requests::MergeRequest;
use rusqlite::Connection;
use serde_json::Value;

/// The MR's own `approvals_before_merge` (deprecated in GitLab 12.3; `null` unless set through
/// the MR create, update or `POST /approvals` endpoints).
pub fn mr_approvals_before_merge(_c: &Connection, _mr: &MergeRequest) -> Result<Option<i64>> {
    Ok(None)
}

/// `approval_rules` in MR webhook payloads (`Gitlab::HookData::MergeRequestBuilder`, EE).
pub fn hook_rules(_c: &Connection, _mr: &MergeRequest) -> Result<Vec<Value>> {
    Ok(vec![])
}

/// The project's `approvals_before_merge` (deprecated; `0` by default).
pub fn project_approvals_before_merge(_c: &Connection, _p: &Project) -> Result<i64> {
    Ok(0)
}
