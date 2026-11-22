//! EE approval policies on merge requests over GraphQL: `policyViolations`,
//! `policiesOverridingApprovalSettings` and `policyBypassStatuses`, and the
//! `dismissPolicyViolations` and `mergeRequestBypassSecurityPolicy` mutations. Recorded on EE
//! 18.11.14 (`fixtures-ee/approval_policies.json`).

use super::exec::{Ctx, FResult, Out};
use super::resolvers::{JsonObj, gid};
use labrat_core::model::Project;
use labrat_core::services::merge_requests::MergeRequest;
use labrat_core::services::policies::{self as pol, Outcome};
use serde_json::{Value, json};

fn obj(ty: &str, v: Value) -> Out {
    Out::obj(JsonObj::new(ty, v))
}

fn list(ty: &str, items: Vec<Value>) -> Out {
    Out::List(items.into_iter().map(|v| obj(ty, v)).collect())
}

/// `ApprovalReportType` values.
fn report_enum(o: &Outcome) -> String {
    o.report_type().to_uppercase()
}

/// `PolicyViolationDetails`: the policies the MR violates (or that failed open), with what
/// violated them and the errors that kept rules from being evaluated.
pub fn violations(cx: &Ctx, p: &Project, mr: &MergeRequest) -> FResult<Out> {
    let c = cx.c;
    let outs = pol::outcomes(c, p, mr)?;
    let mut policies = vec![];
    let mut any_mr = vec![];
    let mut errors = vec![];
    let (mut new_scan, mut prev_scan, mut licenses, mut pipelines) = (vec![], vec![], vec![], vec![]);
    for o in &outs {
        let (dismissed, _) = pol::mr_state(c, mr.id, o.policy.id)?;
        policies.push(json!({
            "dismissed": dismissed,
            "enforcementType": if o.policy.warn() { "WARN" } else { "ENFORCE" },
            "name": o.policy.name,
            "reportType": report_enum(o),
            "securityPolicyId": o.policy.id.to_string(),
            "status": o.status,
        }));
        if let Some(commits) = &o.commits {
            any_mr.push(json!({"name": o.policy.name, "commits": commits}));
        }
        errors.extend(o.errors.iter().cloned());
        new_scan.extend(o.list("newScanFinding"));
        prev_scan.extend(o.list("previousScanFinding"));
        licenses.extend(o.list("licenseScanning"));
        pipelines.extend(o.list("comparisonPipelines"));
    }
    let count = any_mr.len() + new_scan.len() + prev_scan.len() + licenses.len();
    Ok(Out::obj(Details { count, policies, any_mr, errors, new_scan, prev_scan, licenses, pipelines }))
}

struct Details {
    count: usize,
    policies: Vec<Value>,
    any_mr: Vec<Value>,
    errors: Vec<Value>,
    new_scan: Vec<Value>,
    prev_scan: Vec<Value>,
    licenses: Vec<Value>,
    pipelines: Vec<Value>,
}

impl super::exec::Object for Details {
    fn typename(&self) -> String {
        "PolicyViolationDetails".into()
    }
    fn resolve(&self, _cx: &Ctx, field: &str, _args: &super::exec::Args) -> FResult<Out> {
        Ok(match field {
            "violationsCount" => Out::val(self.count),
            "policies" => list("PolicyViolationInfo", self.policies.clone()),
            "anyMergeRequest" => list("PolicyAnyMergeRequestViolation", self.any_mr.clone()),
            "newScanFinding" => list("PolicyScanFindingViolation", self.new_scan.clone()),
            "previousScanFinding" => list("PolicyScanFindingViolation", self.prev_scan.clone()),
            "licenseScanning" => list("PolicyLicenseScanningViolation", self.licenses.clone()),
            "errors" => list("PolicyError", self.errors.clone()),
            "comparisonPipelines" => list("PolicyComparisonPipeline", self.pipelines.clone()),
            _ => return Err(super::resolvers::not_implemented("PolicyViolationDetails", field)),
        })
    }
}

/// `policiesOverridingApprovalSettings`: the project's policies that turn approval settings
/// on, with the settings they turn on.
pub fn overriding(cx: &Ctx, p: &Project) -> FResult<Out> {
    let mut items = vec![];
    for policy in pol::project_policies(cx.c, p.id)? {
        let settings = policy.true_settings();
        if settings.is_empty() {
            continue;
        }
        let name: String = policy.name.split(' ').collect::<Vec<_>>().join("+");
        let edit = cx.req.app.config.url(&format!("/{}/-/security/policies/{}/edit?type=approval_policy", p.full_path, urlencode(&name)));
        items.push(json!({"name": policy.name, "editPath": edit, "settings": Value::Object(settings)}));
    }
    Ok(list("PolicyApprovalSettingsOverride", items))
}

/// Form-encodes a path segment (spaces already turned into `+`).
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'+' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `policyBypassStatuses`: the policies covering the MR that can be bypassed, whether the
/// viewer may and whether someone did.
pub fn bypass_statuses(cx: &Ctx, p: &Project, mr: &MergeRequest) -> FResult<Out> {
    let mut items = vec![];
    let mut seen = vec![];
    for o in pol::outcomes(cx.c, p, mr)? {
        if !o.policy.has_bypass_settings() || seen.contains(&o.policy.id) {
            continue;
        }
        seen.push(o.policy.id);
        let allow = match cx.user() {
            Some(u) => pol::can_bypass(cx.c, p, &o.policy, u)?,
            None => false,
        };
        let (_, bypassed) = pol::mr_state(cx.c, mr.id, o.policy.id)?;
        items.push(json!({"id": gid("Security::Policy", o.policy.id), "name": o.policy.name, "allowBypass": allow, "bypassed": bypassed}));
    }
    Ok(list("PolicyBypassStatus", items))
}

/// The numeric policy ids a mutation names (`securityPolicyIds`); global ids don't match.
pub fn policy_ids(input: &super::exec::Args) -> Vec<i64> {
    input
        .get("securityPolicyIds")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string).or_else(|| x.as_i64().map(|n| n.to_string()))).filter_map(|s| s.parse().ok()).collect())
        .unwrap_or_default()
}
