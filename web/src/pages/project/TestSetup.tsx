// labrat's test setup for a project, through the `/-/labrat/` test endpoints: fake CI, and the
// EE code owners and approval policies that labrat takes instead of files in the repository.

import { useEffect, useState, type ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { labrat } from '../../api/client';
import { ErrorBox, Loading, useToast } from '../../components/ui';
import { EeBadge, EeOnly, useEdition } from '../../edition';
import { useProject, useProjectCrumbs } from './ProjectRoutes';
import { useRefs } from './refs';

const pretty = (v: unknown) => JSON.stringify(v, null, 2);

function Section({ title, ee, help, children }: { title: string; ee?: boolean; help: ReactNode; children: ReactNode }) {
  return (
    <section style={{ padding: '16px 0 24px', borderBottom: '1px solid var(--border)' }}>
      <h2 style={{ fontSize: '1.15rem' }}>
        {title}
        {ee && <EeBadge />}
      </h2>
      <p className="subtle" style={{ marginTop: 4 }}>
        {help}
      </p>
      <div style={{ marginTop: 12 }}>{children}</div>
    </section>
  );
}

/** A JSON body to send, with its parse error shown before sending. */
function JsonEditor({ value, onChange, rows = 12, placeholder }: { value: string; onChange: (v: string) => void; rows?: number; placeholder?: string }) {
  let error: string | null = null;
  try {
    if (value.trim()) JSON.parse(value);
  } catch (e) {
    error = (e as Error).message;
  }
  return (
    <>
      <textarea className="mono" rows={rows} spellCheck={false} value={value} placeholder={placeholder} onChange={(e) => onChange(e.target.value)} />
      {error && <div className="help" style={{ color: 'var(--red-600)' }}>Invalid JSON: {error}</div>}
    </>
  );
}

const parse = (s: string) => JSON.parse(s || 'null');

interface Job {
  id: number;
  pipeline_id: number;
  name: string;
  ref: string | null;
  sha: string;
  status: string;
}

function FakeCi() {
  const { project } = useProject();
  const qc = useQueryClient();
  const toast = useToast();
  const base = `/projects/${project.id}/ci`;
  const cfg = useQuery({ queryKey: ['labrat-ci', project.id], queryFn: () => labrat<{ enabled: boolean }>('GET', base) });
  const jobs = useQuery({ queryKey: ['labrat-ci-jobs', project.id], queryFn: () => labrat<Job[]>('GET', `${base}/jobs`), refetchInterval: 3000 });
  const [text, setText] = useState('');
  useEffect(() => {
    if (cfg.data) setText(pretty(cfg.data.enabled ? cfg.data : { enabled: true, jobs: ['test'], runner: { mode: 'manual' } }));
  }, [cfg.data]);
  const done = () => {
    qc.invalidateQueries({ queryKey: ['labrat-ci', project.id] });
    qc.invalidateQueries({ queryKey: ['labrat-ci-jobs', project.id] });
    qc.invalidateQueries({ queryKey: ['mr', project.id] });
  };
  const save = useMutation({
    mutationFn: (body: unknown) => labrat('PUT', base, body),
    onSuccess: () => {
      toast('Fake CI saved');
      done();
    },
  });
  const run = useMutation({
    mutationFn: ({ action, ref, status }: { action: 'pick' | 'finish'; ref: string; status?: string }) =>
      labrat('POST', `/ci/${action}`, action === 'pick' ? { project_id: project.id, ref } : { project_id: project.id, ref, status, include_pending: true }),
    onSuccess: done,
  });
  const active = (jobs.data ?? []).filter((j) => ['created', 'pending', 'running'].includes(j.status));
  return (
    <Section
      title="Fake CI"
      help={
        <>
          labrat reads no <code>.gitlab-ci.yml</code>. With fake CI on, merge requests get pipelines whose jobs a fake runner finishes, by hand below or automatically (
          <code>runner.mode: "auto"</code> with a <code>status</code> and <code>duration_ms</code>).
        </>
      }
    >
      <ErrorBox error={cfg.error || save.error} />
      {cfg.isLoading ? <Loading /> : <JsonEditor value={text} onChange={setText} rows={8} />}
      <div className="row" style={{ marginTop: 8 }}>
        <button className="btn btn-confirm" disabled={save.isPending} onClick={() => save.mutate(parse(text))}>
          Save
        </button>
        {cfg.data?.enabled && (
          <button className="btn btn-danger-secondary" disabled={save.isPending} onClick={() => save.mutate({ enabled: false })}>
            Turn off
          </button>
        )}
      </div>
      <h3 style={{ fontSize: '1rem', margin: '20px 0 8px' }}>Unfinished jobs</h3>
      <ErrorBox error={jobs.error || run.error} />
      {active.length ? (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Job</th>
                <th>Pipeline</th>
                <th>Ref</th>
                <th>Status</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {active.map((j) => (
                <tr key={j.id}>
                  <td>
                    #{j.id} {j.name}
                  </td>
                  <td>#{j.pipeline_id}</td>
                  <td className="mono small">{j.ref}</td>
                  <td>{j.status}</td>
                  <td style={{ textAlign: 'right', whiteSpace: 'nowrap' }}>
                    {j.status !== 'running' && (
                      <button className="btn btn-sm" disabled={run.isPending} onClick={() => run.mutate({ action: 'pick', ref: j.ref ?? '' })}>
                        Start
                      </button>
                    )}{' '}
                    <button className="btn btn-sm btn-success" disabled={run.isPending} onClick={() => run.mutate({ action: 'finish', ref: j.ref ?? '', status: 'success' })}>
                      Pass
                    </button>{' '}
                    <button className="btn btn-sm btn-danger-secondary" disabled={run.isPending} onClick={() => run.mutate({ action: 'finish', ref: j.ref ?? '', status: 'failed' })}>
                      Fail
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <p className="subtle">No unfinished jobs.</p>
      )}
    </Section>
  );
}

const CODE_OWNERS_EXAMPLE = pretty([
  { name: null, entries: [{ pattern: '*', owners: ['@alice'] }] },
  { name: 'Backend', approvals: 2, optional: false, entries: [{ pattern: '/app/', owners: ['@bob', '@my-group'] }] },
]);

function CodeOwners() {
  const { project } = useProject();
  const { ee } = useEdition();
  const toast = useToast();
  const qc = useQueryClient();
  const refs = useRefs(project.id);
  const [branch, setBranch] = useState(project.default_branch ?? 'main');
  const base = `/projects/${project.id}/code_owners`;
  const q = useQuery({
    queryKey: ['labrat-code-owners', project.id, branch],
    queryFn: () => labrat<{ sections: unknown[] }>('GET', base, undefined, { branch }),
    enabled: ee === true,
  });
  const [text, setText] = useState('');
  useEffect(() => {
    if (q.data) setText(q.data.sections.length ? pretty(q.data.sections) : '');
  }, [q.data]);
  const done = (msg: string) => {
    toast(msg);
    qc.invalidateQueries({ queryKey: ['labrat-code-owners', project.id] });
    qc.invalidateQueries({ queryKey: ['mr', project.id] });
  };
  const save = useMutation({ mutationFn: () => labrat('PUT', base, { branch, sections: parse(text) ?? [] }), onSuccess: () => done('Code owners saved') });
  const remove = useMutation({ mutationFn: () => labrat('DELETE', base, undefined, { branch }), onSuccess: () => done('Code owners removed') });
  return (
    <Section
      title="Code owners"
      ee
      help={
        <>
          labrat reads no <code>CODEOWNERS</code> file. Set a branch's code owners here as the file's sections: a name (<code>null</code> for the unnamed one), the approvals it
          needs, whether it's optional, and patterns with their owners (<code>@user</code>, <code>@group/path</code>). Approval counts only on protected branches that require code
          owner approval (Settings › Repository).
        </>
      }
    >
      <EeOnly>
        <ErrorBox error={q.error || save.error || remove.error} />
        <label className="field" style={{ maxWidth: 300 }}>
          <span className="label">Branch</span>
          <select value={branch} onChange={(e) => setBranch(e.target.value)}>
            {(refs.data?.branches ?? [{ name: branch }]).map((b) => (
              <option key={b.name} value={b.name}>
                {b.name}
              </option>
            ))}
          </select>
        </label>
        <JsonEditor value={text} onChange={setText} placeholder={CODE_OWNERS_EXAMPLE} />
        <div className="row" style={{ marginTop: 8 }}>
          <button className="btn btn-confirm" disabled={save.isPending} onClick={() => save.mutate()}>
            Save
          </button>
          <button className="btn btn-danger-secondary" disabled={remove.isPending || !q.data?.sections.length} onClick={() => remove.mutate()}>
            Remove
          </button>
        </div>
      </EeOnly>
    </Section>
  );
}

const POLICIES_EXAMPLE = pretty([
  {
    name: 'Two approvals',
    description: '',
    enabled: true,
    rules: [{ type: 'any_merge_request', branch_type: 'protected', commits: 'any' }],
    actions: [{ type: 'require_approval', approvals_required: 2, role_approvers: ['maintainer'] }],
    approval_settings: { prevent_approval_by_author: true },
  },
]);

function ApprovalPolicies() {
  const { project } = useProject();
  const { ee } = useEdition();
  const toast = useToast();
  const qc = useQueryClient();
  const base = `/projects/${project.id}/approval_policies`;
  const q = useQuery({ queryKey: ['labrat-policies', project.id], queryFn: () => labrat<{ approval_policy: unknown[] }>('GET', base), enabled: ee === true });
  const [text, setText] = useState('');
  useEffect(() => {
    if (q.data) setText(q.data.approval_policy.length ? pretty(q.data.approval_policy) : '');
  }, [q.data]);
  const done = (msg: string) => {
    toast(msg);
    qc.invalidateQueries({ queryKey: ['labrat-policies', project.id] });
    qc.invalidateQueries({ queryKey: ['mr', project.id] });
  };
  const save = useMutation({ mutationFn: () => labrat('PUT', base, { approval_policy: parse(text) ?? [] }), onSuccess: () => done('Approval policies saved') });
  const remove = useMutation({ mutationFn: () => labrat('DELETE', base), onSuccess: () => done('Approval policies removed') });
  return (
    <Section
      title="Approval policies"
      ee
      help={
        <>
          labrat links no security policy project. Set the project's approval policies here: the <code>approval_policy</code> list of a policy file, in GitLab's schema as JSON.
          Open merge requests are evaluated again at once.
        </>
      }
    >
      <EeOnly>
        <ErrorBox error={q.error || save.error || remove.error} />
        <JsonEditor value={text} onChange={setText} rows={16} placeholder={POLICIES_EXAMPLE} />
        <div className="row" style={{ marginTop: 8 }}>
          <button className="btn btn-confirm" disabled={save.isPending} onClick={() => save.mutate()}>
            Save
          </button>
          <button className="btn btn-danger-secondary" disabled={remove.isPending || !q.data?.approval_policy.length} onClick={() => remove.mutate()}>
            Remove all
          </button>
        </div>
      </EeOnly>
    </Section>
  );
}

const RESULTS_EXAMPLE = pretty({
  'Two approvals': { violated: true, status: 'FAILED', newScanFinding: [{ name: 'SQL injection', path: 'app.rb', reportType: 'SAST', severity: 'CRITICAL', location: {} }] },
});

function PolicyResults() {
  const { project } = useProject();
  const toast = useToast();
  const qc = useQueryClient();
  const [iid, setIid] = useState('');
  const [text, setText] = useState('');
  const save = useMutation({
    mutationFn: () => labrat('PUT', `/projects/${project.id}/merge_requests/${iid}/policy_results`, { policies: parse(text) ?? {} }),
    onSuccess: () => {
      toast('Policy results saved');
      qc.invalidateQueries({ queryKey: ['mr', project.id, Number(iid)] });
    },
  });
  return (
    <Section
      title="Policy results"
      ee
      help={
        <>
          Stands for scan results: sets approval policies as violated or not on one merge request, by policy name (<code>null</code> goes back to labrat's own evaluation), with
          finding details in GraphQL's <code>PolicyViolationDetails</code> shapes.
        </>
      }
    >
      <EeOnly>
        <ErrorBox error={save.error} />
        <label className="field" style={{ maxWidth: 200 }}>
          <span className="label">Merge request IID</span>
          <input type="number" min={1} value={iid} onChange={(e) => setIid(e.target.value)} />
        </label>
        <JsonEditor value={text} onChange={setText} rows={10} placeholder={RESULTS_EXAMPLE} />
        <div className="row" style={{ marginTop: 8 }}>
          <button className="btn btn-confirm" disabled={save.isPending || !iid || !text.trim()} onClick={() => save.mutate()}>
            Save
          </button>
          {iid && <Link to={`/${project.path_with_namespace}/-/merge_requests/${iid}`}>Open !{iid}</Link>}
        </div>
      </EeOnly>
    </Section>
  );
}

const EXAMPLE_RUN = {
  ref: 'main',
  jobs: [
    { name: 'build', stage: 'build', status: 'success', duration: 42, log: 'Building\nDone\n', artifacts: [{ file_type: 'archive', files: { 'dist/app.txt': 'app\n' }, expire_in: '1 week' }] },
    { name: 'unit', stage: 'test', status: 'running', log: 'Running tests\n' },
    { name: 'lint', stage: 'test', status: 'failed', allow_failure: true, log: 'lint: 2 problems\n' },
    { name: 'deploy', stage: 'deploy', when: 'manual' },
  ],
};

interface Run {
  id: number;
  iid: number;
  ref: string;
  status: string;
  jobs: { id: number; name: string; stage: string; status: string; retried: boolean }[];
}

/** Pipeline runs through `/-/labrat/projects/:id/pipelines`. */
function PipelineRuns() {
  const { project } = useProject();
  const qc = useQueryClient();
  const toast = useToast();
  const base = `/projects/${project.id}/pipelines`;
  const runs = useQuery({ queryKey: ['labrat-runs', project.id], queryFn: () => labrat<Run[]>('GET', base), refetchInterval: 5000 });
  const [text, setText] = useState(pretty(EXAMPLE_RUN));
  const [target, setTarget] = useState<number | ''>('');
  const send = useMutation({
    mutationFn: (body: unknown) => (target ? labrat('PUT', `${base}/${target}`, body) : labrat('POST', base, body)),
    onSuccess: () => {
      toast(target ? 'Pipeline updated' : 'Pipeline created');
      qc.invalidateQueries();
    },
  });
  return (
    <Section
      title="Pipeline runs"
      help={
        <>
          labrat runs no CI: describe a pipeline run, and GitLab&apos;s APIs report it like a real one. POST creates a run (<code>ref</code> or <code>merge_request_iid</code>,{' '}
          <code>stages</code>, <code>jobs</code> with <code>status</code>, <code>duration</code>, <code>log</code>, <code>artifacts</code>…); PUT moves an existing run on, matching
          jobs by name (<code>log_append</code> adds to a log). Jobs without a status are processed as GitLab would.
        </>
      }
    >
      <ErrorBox error={send.error || runs.error} />
      <div className="row" style={{ marginBottom: 8, gap: 8 }}>
        <label>
          Send to{' '}
          <select value={target} onChange={(e) => setTarget(e.target.value ? Number(e.target.value) : '')}>
            <option value="">a new pipeline (POST)</option>
            {(runs.data ?? []).map((r) => (
              <option key={r.id} value={r.id}>
                pipeline #{r.id} ({r.ref}, {r.status}) (PUT)
              </option>
            ))}
          </select>
        </label>
      </div>
      <JsonEditor value={text} onChange={setText} rows={14} />
      <div className="row" style={{ marginTop: 8 }}>
        <button className="btn btn-confirm" disabled={send.isPending} onClick={() => send.mutate(parse(text))}>
          {target ? 'Update pipeline' : 'Create pipeline'}
        </button>
        <Link className="btn" to={`/${project.path_with_namespace}/-/pipelines`}>
          Pipelines
        </Link>
      </div>
    </Section>
  );
}

export function TestSetup() {
  useProjectCrumbs([{ label: 'Settings' }, { label: 'labrat test setup' }]);
  return (
    <>
      <div className="page-head">
        <h1>labrat test setup</h1>
      </div>
      <p className="subtle">
        Things GitLab gets from CI and from files in the repository, set directly. These go through labrat's test endpoints under <code>/-/labrat/</code>, not GitLab's API.
      </p>
      <PipelineRuns />
      <FakeCi />
      <CodeOwners />
      <ApprovalPolicies />
      <PolicyResults />
    </>
  );
}
