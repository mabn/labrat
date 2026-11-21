//! EE approval types for GraphQL: `MergeRequestApprovalState`, `ApprovalRule` and
//! `ApprovalProjectRule`, and the approval rule mutations. Recorded on EE 18.11.14
//! (`fixtures-ee/graphql_approvals.json`).

use super::exec::{Args, Ctx, FResult, Object, Out};
use super::resolvers::{Conn, NamespaceObj, Shared, gid, user_conn};
use labrat_core::model::{Namespace, Project, User};
use labrat_core::perms::GroupPerms;
use labrat_core::services::approvals::{self as ap, Rule, Scope, Wrapped};
use labrat_core::services::merge_requests::MergeRequest;
use serde_json::{Value, json};
use std::rc::Rc;

fn rule_type(r: &Rule) -> String {
    r.rule_type.to_uppercase()
}

fn rule_gid(r: &Rule) -> Value {
    match r.scope {
        Scope::Project => gid("ApprovalProjectRule", r.id),
        Scope::MergeRequest => gid("ApprovalMergeRequestRule", r.id),
    }
}

/// `MergeRequestApprovalState`.
pub struct ApprovalStateObj {
    pub p: Project,
    pub mr: MergeRequest,
}

/// A rule is invalid when it requires more approvals than it has eligible approvers; such a rule
/// counts as approved (`allowMergeWhenInvalid`).
fn invalid(w: &Wrapped) -> bool {
    !w.rule.is_any_approver() && (w.approvers.len() as i64) < w.rule.approvals_required
}

impl Object for ApprovalStateObj {
    fn typename(&self) -> String {
        "MergeRequestApprovalState".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let st = ap::state(cx.c, &self.p, &self.mr)?;
        Ok(match field {
            "approvalRulesOverwritten" => Out::val(st.overwritten),
            "rules" => Out::List(
                st.rules
                    .iter()
                    .map(|w| Out::obj(ApprovalRuleObj { w: w.clone(), overwritten: st.overwritten, p: self.p.clone() }))
                    .collect(),
            ),
            "invalidApproversRules" => Out::List(
                st.rules
                    .iter()
                    .filter(|w| invalid(w))
                    .map(|w| Out::obj(ApprovalRuleObj { w: w.clone(), overwritten: st.overwritten, p: self.p.clone() }))
                    .collect(),
            ),
            "suggestedApprovers" => user_conn(cx, &st.suggested_approvers(), "UserCoreConnection", "UserCore", None, args)?,
            _ => return Err(super::resolvers::not_implemented("MergeRequestApprovalState", field)),
        })
    }
}

/// `ApprovalRule`: a rule as an MR's approval state sees it.
pub struct ApprovalRuleObj {
    w: Wrapped,
    overwritten: bool,
    p: Project,
}

fn groups_conn(cx: &Ctx, ids: &[i64], args: &Args) -> FResult<Out> {
    let mut items = vec![];
    for id in ids {
        if let Some(n) = Namespace::find(cx.c, *id)?
            && GroupPerms::load(cx.c, cx.user(), &n)?.read_group()
        {
            items.push((json!({"id": id.to_string()}), Shared(Rc::new(NamespaceObj::group(n)))));
        }
    }
    Ok(Out::obj(Conn::new("GroupConnection", items, args)?))
}

fn hidden_groups(cx: &Ctx, ids: &[i64]) -> FResult<bool> {
    for id in ids {
        if let Some(n) = Namespace::find(cx.c, *id)?
            && !GroupPerms::load(cx.c, cx.user(), &n)?.read_group()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

impl Object for ApprovalRuleObj {
    fn typename(&self) -> String {
        "ApprovalRule".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let w = &self.w;
        Ok(match field {
            "id" => Out::val(rule_gid(&w.rule)),
            "name" => Out::val(w.name()),
            "type" => Out::val(rule_type(&w.rule)),
            "approvalsRequired" => Out::val(w.rule.approvals_required),
            "approved" => Out::val(w.approved() || invalid(w)),
            "overridden" => Out::val(self.overwritten && w.overridden()),
            "section" => Out::Null,
            "containsHiddenGroups" => Out::val(hidden_groups(cx, &w.rule.group_ids)?),
            "invalid" => Out::val(invalid(w)),
            "allowMergeWhenInvalid" => Out::val(true),
            "scanResultPolicies" => Out::List(vec![]),
            "eligibleApprovers" => {
                let mut out = vec![];
                for id in &w.approvers {
                    if let Some(u) = User::find(cx.c, *id)? {
                        out.push(Out::obj(super::resolvers::UserObj::new("UserCore", u)));
                    }
                }
                Out::List(out)
            }
            "users" => user_conn(cx, &w.rule.user_ids, "UserCoreConnection", "UserCore", None, args)?,
            "groups" => groups_conn(cx, &w.rule.group_ids, args)?,
            "approvedBy" => user_conn(cx, &w.approved_by, "UserCoreConnection", "UserCore", None, args)?,
            "commentedBy" => user_conn(cx, &[], "UserCoreConnection", "UserCore", None, args)?,
            "sourceRule" => match (&w.source, self.overwritten) {
                (Some(s), true) => Out::obj(SourceRuleObj { r: s.clone(), p: self.p.clone() }),
                _ => Out::Null,
            },
            _ => return Err(super::resolvers::not_implemented("ApprovalRule", field)),
        })
    }
}

/// A project rule shown as an MR rule's `sourceRule` (an `ApprovalRule`).
struct SourceRuleObj {
    r: Rule,
    p: Project,
}

impl Object for SourceRuleObj {
    fn typename(&self) -> String {
        "ApprovalRule".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        let approvers = ap::rule_approver_ids(cx.c, &self.r)?;
        let w = Wrapped { rule: self.r.clone(), source: None, approvers, approved_by: vec![] };
        ApprovalRuleObj { w, overwritten: false, p: self.p.clone() }.resolve(cx, field, args).map(|o| match (field, o) {
            ("name", _) => Out::val(self.r.name.clone()),
            (_, o) => o,
        })
    }
}

/// `ApprovalProjectRule` (branch rules and the project rule mutations).
pub struct ApprovalProjectRuleObj {
    pub rule: Rule,
}

impl Object for ApprovalProjectRuleObj {
    fn typename(&self) -> String {
        "ApprovalProjectRule".into()
    }
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out> {
        Ok(match field {
            "id" => Out::val(gid("ApprovalProjectRule", self.rule.id)),
            "name" => Out::val(self.rule.name.clone()),
            "type" => Out::val(rule_type(&self.rule)),
            "approvalsRequired" => Out::val(self.rule.approvals_required),
            // A users relation paginated by id, newest first.
            "eligibleApprovers" => {
                let mut ids = ap::rule_approver_ids(cx.c, &self.rule)?;
                ids.sort_by(|a, b| b.cmp(a));
                user_conn(cx, &ids, "UserCoreConnection", "UserCore", None, args)?
            }
            _ => return Err(super::resolvers::not_implemented("ApprovalProjectRule", field)),
        })
    }
}

/// The project rules a branch rule shows: for "All branches", the rules without branches; for
/// a protected branch, the rules scoped to it. Newest first.
pub fn branch_rule_approval_rules(cx: &Ctx, p: &Project, protected_branch: Option<i64>, args: &Args) -> FResult<Out> {
    let mut rules: Vec<Rule> = vec![];
    for r in ap::project_rules(cx.c, p)? {
        let ids = ap::rule_protected_branch_ids(cx.c, p, &r)?;
        let keep = match protected_branch {
            None => ids.is_empty() && !r.applies_to_all_protected_branches,
            Some(pb) => ids.contains(&pb),
        };
        if keep {
            rules.push(r);
        }
    }
    rules.sort_by(|a, b| b.id.cmp(&a.id));
    let items = rules
        .into_iter()
        .map(|r| (json!({"id": r.id.to_string()}), Shared(Rc::new(ApprovalProjectRuleObj { rule: r }))))
        .collect();
    Ok(Out::obj(Conn::new("ApprovalProjectRuleConnection", items, args)?))
}

/// ActiveModel full messages (`Name can't be blank`).
fn full_messages(errs: &[(String, String)]) -> Vec<String> {
    errs.iter()
        .map(|(f, m)| {
            let mut name = f.replace('_', " ");
            if let Some(c) = name.get(0..1) {
                name = c.to_uppercase() + &name[1..];
            }
            format!("{name} {m}")
        })
        .collect()
}

/// Plain numeric ids from an `[ID!]`/`[String!]` argument; global ids are not accepted.
fn numeric_ids(input: &Args, key: &str) -> Option<Vec<i64>> {
    input.get(key).and_then(Value::as_array).map(|a| {
        a.iter()
            .filter_map(|v| match v {
                Value::String(s) => s.parse().ok(),
                Value::Number(n) => n.as_i64(),
                _ => None,
            })
            .collect()
    })
}

/// Approvers from the arguments, keeping project members and visible groups.
fn approver_params(cx: &Ctx, p: &Project, input: &Args, rp: &mut ap::RuleParams) -> FResult<()> {
    if let Some(ids) = numeric_ids(input, "userIds") {
        let given = input.get("userIds").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
        let mut keep = vec![];
        for id in ids {
            if ap::project_member(cx.c, p, id)? && !keep.contains(&id) {
                keep.push(id);
            }
        }
        rp.user_ids = Some(keep);
        rp.approvers_given |= given;
    }
    if let Some(ids) = numeric_ids(input, "groupIds") {
        let given = input.get("groupIds").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
        let mut keep = vec![];
        for id in ids {
            if let Some(n) = Namespace::find(cx.c, id)?.filter(Namespace::is_group)
                && GroupPerms::load(cx.c, cx.user(), &n)?.read_group()
            {
                keep.push(n.id);
            }
        }
        rp.group_ids = Some(keep);
        rp.approvers_given |= given;
    }
    Ok(())
}

/// The approval rule mutations. Returns the payload's `errors` and `approvalRule`.
pub fn mutation(cx: &Ctx, field: &str, input: &Args) -> FResult<(Vec<String>, Option<Rule>)> {
    use super::resolvers::{parse_gid, unavailable};
    let user = cx.user().ok_or_else(unavailable)?.clone();
    let c = cx.c;
    match field {
        "mergeRequestUpdateApprovalRule" => {
            let path = input.str("projectPath").unwrap_or_default();
            let p = Project::find_by_full_path(c, &path)?.ok_or_else(unavailable)?;
            let perms = labrat_core::perms::ProjectPerms::load(c, Some(&user), &p)?;
            let iid: i64 = input.str("iid").and_then(|s| s.parse().ok()).unwrap_or(0);
            let mr = MergeRequest::find_by_iid(c, p.id, iid)?.ok_or_else(unavailable)?;
            let allowed = perms.read_merge_request(&p) && (perms.admin || perms.admin_project() || (mr.author_id == user.id && perms.developer()));
            if !allowed || !ap::effective(c, &p)?.allow_overrides_to_approver_list_per_merge_request.value {
                return Err(unavailable());
            }
            // An unknown rule raises in GitLab: a 500.
            let Some(rule) = ap::mr_rule(c, &mr, input.int("approvalRuleId").unwrap_or(0))? else {
                super::resolvers::INTERNAL_ERROR.with(|f| f.set(true));
                return Ok((vec![], None));
            };
            let mut rp = ap::RuleParams { name: input.str("name"), approvals_required: input.int("approvalsRequired"), ..Default::default() };
            approver_params(cx, &p, input, &mut rp)?;
            // Validation errors are dropped; the payload never carries the MR.
            let _ = ap::update_rule(c, &rule, &rp)?;
            Ok((vec![], None))
        }
        "branchRuleApprovalProjectRuleCreate" => {
            let raw = input.str("branchRuleId").unwrap_or_default();
            let (p, pb) = if let Ok(id) = parse_gid(&raw, &["Projects::AllBranchesRule"]) {
                (Project::find(c, id)?.ok_or_else(unavailable)?, None)
            } else {
                let id = parse_gid(&raw, &["Projects::BranchRule"])?;
                let pid: Option<i64> = c.query_row("SELECT project_id FROM protected_branches WHERE id = ?1", [id], |r| r.get(0)).ok();
                let p = Project::find(c, pid.ok_or_else(unavailable)?)?.ok_or_else(unavailable)?;
                (p, Some(id))
            };
            if !labrat_core::perms::ProjectPerms::load(c, Some(&user), &p)?.admin_project() {
                return Err(unavailable());
            }
            let mut rp = ap::RuleParams {
                name: input.str("name"),
                approvals_required: input.int("approvalsRequired"),
                protected_branch_ids: pb.map(|id| vec![id]),
                ..Default::default()
            };
            approver_params(cx, &p, input, &mut rp)?;
            match ap::create_rule(c, Scope::Project, p.id, &rp)? {
                Ok(r) => Ok((vec![], Some(r))),
                Err(errs) => Ok((full_messages(&errs), None)),
            }
        }
        _ => {
            let id = parse_gid(&input.str("id").unwrap_or_default(), &["ApprovalProjectRule"])?;
            let pid: Option<i64> = c.query_row("SELECT project_id FROM approval_project_rules WHERE id = ?1", [id], |r| r.get(0)).ok();
            let p = Project::find(c, pid.ok_or_else(unavailable)?)?.ok_or_else(unavailable)?;
            if !labrat_core::perms::ProjectPerms::load(c, Some(&user), &p)?.admin_project() {
                return Err(unavailable());
            }
            let rule = ap::project_rule(c, &p, id)?.ok_or_else(unavailable)?;
            if field == "approvalProjectRuleDelete" {
                ap::delete_rule(c, &rule)?;
                return Ok((vec![], Some(rule)));
            }
            let mut rp = ap::RuleParams { name: input.str("name"), approvals_required: input.int("approvalsRequired"), ..Default::default() };
            approver_params(cx, &p, input, &mut rp)?;
            match ap::update_rule(c, &rule, &rp)? {
                Ok(r) => Ok((vec![], Some(r))),
                Err(errs) => Ok((full_messages(&errs), None)),
            }
        }
    }
}
