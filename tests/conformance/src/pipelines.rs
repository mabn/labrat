//! CI pipelines (roadmap phase 7): a scenario describes a pipeline's jobs, and then moves its
//! jobs on one runner action at a time.
//!
//! - Recording: the project gets a `.gitlab-ci.yml` with those jobs (scripts that only `echo`,
//!   never run), and the harness plays the runner through the runner API: it picks jobs,
//!   sends their logs, uploads artifacts and finishes them.
//! - Replaying: labrat's test endpoints (`/-/labrat/projects/:id/pipelines`) create the same
//!   pipeline and move its jobs the same way.

use crate::{Res, Scenario, runner_info};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::HashMap;

/// The pipeline a scenario is moving on.
#[derive(Default)]
pub(crate) struct CiRun {
    project: i64,
    /// The pipeline's id on the instance being driven.
    pipeline: i64,
    /// Recording: each job's latest attempt as `(job id, job token)`, by name.
    jobs: HashMap<String, (i64, String)>,
    /// Recording: how much of each job's log was sent.
    sent: HashMap<String, usize>,
}

/// The `.gitlab-ci.yml` for `jobs`:
/// `[{"name", "stage", "when", "allow_failure", "coverage", "manual_confirmation"}]`.
pub fn ci_yaml(jobs: &Value) -> String {
    let mut stages: Vec<String> = vec![];
    for j in jobs.as_array().expect("jobs") {
        let s = j["stage"].as_str().unwrap_or("test").to_string();
        if !stages.contains(&s) {
            stages.push(s);
        }
    }
    let mut y = format!("stages: [{}]\n", stages.join(", "));
    for j in jobs.as_array().unwrap() {
        let name = j["name"].as_str().expect("job name");
        y.push_str(&format!("{name}:\n  stage: {}\n  script: echo {name}\n", j["stage"].as_str().unwrap_or("test")));
        if let Some(w) = j["when"].as_str() {
            y.push_str(&format!("  when: {w}\n"));
        }
        if let Some(a) = j["allow_failure"].as_bool() {
            y.push_str(&format!("  allow_failure: {a}\n"));
        }
        if j["coverage"].as_bool() == Some(true) {
            y.push_str("  coverage: '/Coverage: \\d+\\.\\d+%/'\n");
        }
        if let Some(m) = j["manual_confirmation"].as_str() {
            y.push_str(&format!("  manual_confirmation: \"{m}\"\n"));
        }
    }
    y
}

impl Scenario {
    fn ci_run(&mut self) -> &mut CiRun {
        self.ci_run.as_mut().expect("ci_pipeline first")
    }

    /// Starts a pipeline for a push of a `.gitlab-ci.yml` with `jobs` to `branch` (created
    /// from `start_branch` when given): the commit is a recorded step. See `ci_yaml` for the
    /// job fields.
    pub async fn ci_pipeline(&mut self, label: &str, project: i64, token: &str, branch: &str, start_branch: Option<&str>, jobs: Value) -> Res {
        let enc = branch.replace('/', "%2F");
        let before = self
            .send(Method::GET, &format!("/projects/{project}/repository/branches/{enc}"), Some(token), Value::Null)
            .await
            .body["commit"]["id"]
            .as_str()
            .unwrap_or("0000000000000000000000000000000000000000")
            .to_string();
        let exists = self
            .send(Method::GET, &format!("/projects/{project}/repository/files/.gitlab-ci.yml"), Some(token), json!({"ref": start_branch.unwrap_or(branch)}))
            .await
            .status
            == 200;
        let mut body = json!({"branch": branch, "commit_message": "Set CI", "actions": [{"action": if exists { "update" } else { "create" }, "file_path": ".gitlab-ci.yml", "content": ci_yaml(&jobs)}]});
        if let Some(s) = start_branch {
            body["start_branch"] = json!(s);
        }
        let commit = self.step(label, Method::POST, &format!("/projects/{project}/repository/commits"), Some(token), body, "commit").await;
        let sha = commit.body["id"].as_str().expect("commit id").to_string();
        let pipeline = if self.target.recording {
            let mut id = None;
            for _ in 0..60 {
                let r = self.send(Method::GET, &format!("/projects/{project}/pipelines"), Some(token), json!({"sha": sha})).await;
                if let Some(i) = r.body.as_array().and_then(|a| a.first()).and_then(|p| p["id"].as_i64()) {
                    id = Some(i);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            self.ci_created.push(self.started.elapsed().as_millis() as i64);
            self.settle().await;
            id.expect("GitLab created no pipeline for the push")
        } else {
            let user = self.send(Method::GET, "/user", Some(token), Value::Null).await.body["username"].as_str().unwrap_or("").to_string();
            let spec: Vec<Value> = jobs
                .as_array()
                .unwrap()
                .iter()
                .map(|j| {
                    // Manual jobs may fail unless they say otherwise, as in GitLab.
                    let manual = j["when"] == "manual";
                    let mut job = json!({"name": j["name"], "stage": j["stage"].as_str().unwrap_or("test"), "when": j["when"].as_str().unwrap_or("on_success"),
                           "allow_failure": j["allow_failure"].as_bool().unwrap_or(manual)});
                    if let Some(m) = j["manual_confirmation"].as_str() {
                        job["manual_confirmation"] = json!(m);
                    }
                    job
                })
                .collect();
            // GitLab may take a while to create the pipeline after the push.
            let n = self.ci_created_done;
            self.ci_created_done += 1;
            if let Some(t) = self.expected_ci_created.as_ref().and_then(|t| t.get(n).copied()) {
                self.advance_clock(t).await;
            }
            let url = format!("{}/-/labrat/projects/{project}/pipelines", self.target.base);
            let r = self
                .http
                .post(&url)
                .json(&json!({"ref": branch, "sha": sha, "before_sha": before, "source": "push", "user": user, "jobs": spec}))
                .send()
                .await
                .expect("pipeline request failed");
            let status = r.status();
            let v: Value = r.json().await.unwrap_or(Value::Null);
            assert!(status.is_success(), "creating the labrat pipeline failed: {status} {v}");
            self.settle().await;
            v["id"].as_i64().expect("pipeline id")
        };
        self.ci_run = Some(CiRun { project, pipeline, ..Default::default() });
        self.ci_project = Some(project);
        commit
    }

    /// The id of the pipeline `ci_pipeline` started.
    pub fn ci_pipeline_id(&self) -> i64 {
        self.ci_run.as_ref().expect("ci_pipeline first").pipeline
    }

    /// The id of the latest attempt of the job `name` in the pipeline (an unrecorded request).
    pub async fn ci_job_id(&mut self, token: &str, name: &str) -> i64 {
        let run = self.ci_run.as_ref().expect("ci_pipeline first");
        let path = format!("/projects/{}/pipelines/{}/jobs", run.project, run.pipeline);
        let r = self.send(Method::GET, &path, Some(token), json!({"per_page": 100})).await;
        r.body.as_array().and_then(|a| a.iter().find(|j| j["name"] == name)).and_then(|j| j["id"].as_i64()).unwrap_or_else(|| panic!("no job {name}: {}", r.body))
    }

    /// Replaying: one change to the pipeline's jobs through labrat's test endpoint.
    async fn ci_update(&mut self, job: Value) {
        let run = self.ci_run.as_ref().expect("ci_pipeline first");
        let url = format!("{}/-/labrat/projects/{}/pipelines/{}", self.target.base, run.project, run.pipeline);
        let r = self.http.put(&url).json(&json!({"jobs": [job]})).send().await.expect("pipeline request failed");
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        assert!(status.is_success(), "updating the labrat pipeline failed: {status} {v}");
        self.settle().await;
    }

    /// The runner picks up the job `name` (its pending latest attempt), which starts running.
    /// Recording panics when the runner gets another job: jobs are picked in GitLab's queue
    /// order.
    pub async fn ci_job_start(&mut self, name: &str) {
        self.sync_ci_clock(true).await;
        if self.target.recording {
            let project = self.ci_run().project;
            let token = std::env::var("GITLAB_RUNNER_TOKEN").expect("GITLAB_RUNNER_TOKEN (a runner registered on the reference instance)");
            let url = format!("{}/api/v4/jobs/request", self.target.base);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
            loop {
                let r = self.http.post(&url).json(&json!({"token": token, "info": runner_info()})).send().await.expect("job request failed");
                if r.status().as_u16() != 201 {
                    assert!(std::time::Instant::now() < deadline, "the runner got no job (waiting for {name})");
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                let job: Value = r.json().await.unwrap();
                let id = job["id"].as_i64().unwrap();
                let jtoken = job["token"].as_str().unwrap().to_string();
                let pid = job["variables"].as_array().and_then(|v| v.iter().find(|x| x["key"] == "CI_PROJECT_ID")).and_then(|x| x["value"].as_str()).and_then(|v| v.parse::<i64>().ok());
                if pid != Some(project) {
                    self.ci_put_job(id, &jtoken, "success").await;
                    continue;
                }
                let got = job["job_info"]["name"].as_str().unwrap_or("").to_string();
                assert_eq!(got, name, "the runner picked another job");
                let run = self.ci_run();
                run.jobs.insert(name.to_string(), (id, jtoken));
                run.sent.insert(name.to_string(), 0);
                break;
            }
            self.settle().await;
        } else {
            self.ci_update(json!({"name": name, "status": "running"})).await;
        }
    }

    /// The running job `name` sends more of its log.
    pub async fn ci_job_log(&mut self, name: &str, text: &str) {
        self.sync_ci_clock(false).await;
        if self.target.recording {
            let (id, token) = self.ci_run().jobs.get(name).cloned().unwrap_or_else(|| panic!("{name} isn't running"));
            let start = self.ci_run().sent.get(name).copied().unwrap_or(0);
            let bytes = text.as_bytes().to_vec();
            let url = format!("{}/api/v4/jobs/{id}/trace", self.target.base);
            let r = self
                .http
                .patch(&url)
                .header("JOB-TOKEN", &token)
                .header("Content-Range", format!("{start}-{}", start + bytes.len() - 1))
                .header("Content-Type", "text/plain")
                .body(bytes.clone())
                .send()
                .await
                .expect("trace request failed");
            assert_eq!(r.status().as_u16(), 202, "sending the log of {name}");
            *self.ci_run().sent.entry(name.to_string()).or_default() += bytes.len();
            self.settle().await;
        } else {
            self.ci_update(json!({"name": name, "log_append": text})).await;
        }
    }

    /// The running job `name` uploads an artifacts archive of `files` (as labrat zips them),
    /// expiring in `expire_in` (GitLab's default when `None`).
    pub async fn ci_job_artifacts(&mut self, name: &str, files: &[(&str, &str)], expire_in: Option<&str>) {
        self.sync_ci_clock(false).await;
        if self.target.recording {
            let (id, token) = self.ci_run().jobs.get(name).cloned().unwrap_or_else(|| panic!("{name} isn't running"));
            let zip = labrat_core::zip::write(&files.iter().map(|(p, c)| (p.to_string(), c.as_bytes().to_vec())).collect::<Vec<_>>());
            let part = reqwest::multipart::Part::bytes(zip).file_name("artifacts.zip").mime_str("application/zip").unwrap();
            let mut form = reqwest::multipart::Form::new().part("file", part).text("artifact_type", "archive").text("artifact_format", "zip");
            if let Some(e) = expire_in {
                form = form.text("expire_in", e.to_string());
            }
            let url = format!("{}/api/v4/jobs/{id}/artifacts", self.target.base);
            let r = self.http.post(&url).header("JOB-TOKEN", &token).multipart(form).send().await.expect("artifacts request failed");
            let status = r.status().as_u16();
            assert_eq!(status, 201, "uploading the artifacts of {name}: {}", r.text().await.unwrap_or_default());
            self.settle().await;
        } else {
            let files: serde_json::Map<String, Value> = files.iter().map(|(p, c)| (p.to_string(), json!(c))).collect();
            let mut a = json!({"file_type": "archive", "files": files});
            if let Some(e) = expire_in {
                a["expire_in"] = json!(e);
            }
            self.ci_update(json!({"name": name, "artifacts": [a]})).await;
        }
    }

    /// The running job `name` finishes with `status` (`success`, `failed` or `canceled`;
    /// failures are `script_failure`). `coverage` is what GitLab reads from the job's log
    /// with the job's `coverage` pattern; replaying passes it on.
    pub async fn ci_job_finish(&mut self, name: &str, status: &str, coverage: Option<f64>) {
        self.sync_ci_clock(false).await;
        if self.target.recording {
            let (id, token) = self.ci_run().jobs.get(name).cloned().unwrap_or_else(|| panic!("{name} isn't running"));
            let mut body = json!({"token": token, "state": status, "exit_code": if status == "success" { 0 } else { 1 }});
            if status == "failed" {
                body["failure_reason"] = json!("script_failure");
            }
            let url = format!("{}/api/v4/jobs/{id}", self.target.base);
            let r = self.http.put(&url).json(&body).send().await.expect("job update failed");
            assert!(matches!(r.status().as_u16(), 200 | 403), "finishing job {name}: {}", r.status());
            self.settle().await;
        } else {
            let mut job = json!({"name": name, "status": status, "exit_code": if status == "success" { 0 } else { 1 }});
            if status == "failed" {
                job["failure_reason"] = json!("script_failure");
            }
            if let Some(c) = coverage {
                job["coverage"] = json!(c);
            }
            self.ci_update(job).await;
        }
    }
}
