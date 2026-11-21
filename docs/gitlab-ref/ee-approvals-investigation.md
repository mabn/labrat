# EE approval rules: investigation

What GitLab EE 18.11 exposes for approval rules and settings, and how it differs from CE. The
EE instance is studied only through its API; no EE source or container is used.

- **EE reference:** a GitLab EE 18.11.14 instance (`18.11.14-ee`, Ultimate license) behind
  Cloudflare Access. Its URL, the Access service token and the credentials are kept outside the
  repository, in `~/dev/gitlab-ee-ref/` (`cf.env`, `labrat.env`). It was prepared the same way as
  the CE instance (`setup.sh`): an admin `claude` with a `claude-code` token, and the
  `rebase_on_merge_automatic` flag turned on.
- **CE reference:** the usual 18.11.14 instance at `localhost:8929`. Both run the same patch
  release.
- **Recorded:** 2026-11-21, on a fresh group with a README-initialised project and one MR.

## Endpoints

| Endpoint | EE | CE |
|---|---|---|
| `GET/POST /projects/:id/approvals` | 200 / 201: project approval settings (old field names) | 404 `{"error":"404 Not Found"}` |
| `GET/POST /projects/:id/approval_rules`, `GET/PUT/DELETE …/:rule_id` | rules API | 404 route |
| `GET/PUT /projects/:id/merge_request_approval_setting` | `{value, locked, inherited_from}` per setting | 404 route |
| `GET/PUT /groups/:id/merge_request_approval_setting` | same shape, group level | 404 route |
| `GET/POST/PUT /groups/:id/approval_rules` | 404 `{"message":"404 Not Found"}`: the endpoint exists but is behind the `approval_group_rules` flag, off by default | 404 route (`error` key) |
| `GET /projects/:id/merge_requests/:iid/approvals` | 200, the long EE body (below) | 200, `{user_has_approved, user_can_approve, approved, approved_by}` |
| `POST …/approvals` (set required approvals on the MR) | deprecated EE endpoint | 404 route |
| `GET …/approval_state` and its alias `…/approval_settings` | `{"approval_rules_overwritten":false,"rules":[]}` | 404 route |
| `GET/POST …/approval_rules`, `GET/PUT/DELETE …/:rule_id` | MR-level rules | 404 route |
| `POST …/approve`, `POST …/unapprove` | 201 with the EE approvals body | 201 with the CE body |
| `PUT …/reset_approvals` | bot users only (documented) | exists in CE too; to check |
| `GET /admin/merge_requests_approvals` | 404 route (not in 18.11) | 404 route |

CE answers unknown routes with `{"error":"404 Not Found"}`. EE uses the same body for routes it
doesn't have, and `{"message":"404 Not Found"}` for an existing route that refuses the request.

### MR approvals body on EE (no rules, nobody approved)

```json
{"id":1,"iid":1,"project_id":1,"title":"t","description":null,"state":"opened",
 "created_at":"…","updated_at":"…","merge_status":"can_be_merged","approved":true,
 "approvals_required":0,"approvals_left":0,"require_password_to_approve":false,
 "approved_by":[],"suggested_approvers":[],"approvers":[],"approver_groups":[],
 "user_has_approved":false,"user_can_approve":false,"approval_rules_left":[],
 "has_approval_rules":false,"merge_request_approvers_available":true,
 "multiple_approval_rules_available":true,"invalid_approvers_rules":[]}
```

`approved` is `true` with no rules and no approvals. On CE it is `false` until someone approves.

### Defaults on a new project in a new group (EE)

- `GET /projects/:id/approvals`: `approvals_before_merge: 0`, `reset_approvals_on_push: true`,
  `merge_requests_author_approval: false`, `merge_requests_disable_committers_approval: false`,
  `disable_overriding_approvers_per_merge_request: false`, the rest `false`.
- `GET /projects/:id/merge_request_approval_setting` says the same things with the new names:
  - `allow_author_approval: false`;
  - `allow_committer_approval: true`;
  - `allow_overrides_to_approver_list_per_merge_request: true`;
  - `retain_approvals_on_push: false`.
- `GET /groups/:id/merge_request_approval_setting` on the new group: `allow_author_approval:
  true` and `retain_approvals_on_push: true`. These differ from the project's values, so the
  group's settings don't feed the project defaults until the group sets them.

**The author can't approve their own MR by default on EE.** `POST /approve` as the author
answers `401 {"message":"401 Unauthorized"}`. CE lets the author approve. Many CE scenarios
approve as the author, so EE behaviour can't be the default for them.

## Other responses EE changes

These are all extra fields EE adds compared with CE. Unless noted, each list is in the order the
fields appear.

| Response | Extra EE fields |
|---|---|
| MR (`GET /merge_requests/:iid` and lists) | `approvals_before_merge` |
| Project | `approvals_before_merge`, `mirror`, `compliance_frameworks`, `merge_pipelines_enabled`, `merge_trains_enabled`, `issues_template`, `merge_requests_template`, `merge_request_title_regex` and 20 more |
| Group | 24 fields: `ldap_*`, `duo_*`, `membership_lock`, `ip_restriction_ranges` and others |
| User, `/user` | `shared_runners_minutes_limit`, `is_auditor`, `using_license_seat`, `scim_identities` and others |
| Members | `email`, `membership_state` |
| Protected branches | `unprotect_access_levels`, `code_owner_approval_required`, `inherited`; each access level also has `user_id` and `group_id` |
| `GET /version` | `"version":"18.11.14-ee"`, `"enterprise":true` |

## Webhooks

The EE instance can't reach a receiver on this machine. The hook events API
(`GET /projects/:id/hooks/:hook_id/events`) returns each delivery's `request_data` and
`request_headers`, though, so payloads can be recorded from it. An `approved` MR hook was read
back that way.

## GraphQL

- EE's schema has 3,742 types. labrat embeds CE's, which has 1,669. PLAN.md still quotes "3,742
  types and 610 mutations", which looks like an EE count.
- **Approval fields on `MergeRequest`:** `approvalState`, `approvalsLeft`, `approvalsRequired`,
  plus the policy fields `policiesOverridingApprovalSettings`, `policyBypassStatuses` and
  `policyViolations`. CE has `approved` and `approvedBy` only.
- **Approval mutations:** `mergeRequestUpdateApprovalRule`,
  `branchRuleApprovalProjectRuleCreate`, `approvalProjectRuleUpdate` and
  `approvalProjectRuleDelete`.
- **Policy mutations:** `securityPolicyProjectAssign`, `securityPolicyProjectCreate`,
  `securityPolicyProjectUnassign`, `resyncSecurityPolicies`, `mergeRequestBypassSecurityPolicy`
  and `dismissPolicyViolations`.
- The introspection file can be fetched from the EE instance like CE's
  (`query IntrospectionQuery`); it is about 6.9 MB uncompressed.

## Approval policies

Merge request approval policies (Ultimate) live as YAML in a separate security policy project,
which is linked to the project with `securityPolicyProjectAssign`. GitLab turns them into
`report_approver` rules on MRs.
- The `scan_finding` and `license_finding` rule types depend on CI pipelines and security scanner
  reports, both out of scope.
- The `any_merge_request` rule type, which requires approval on every MR to a protected branch,
  doesn't need CI.
