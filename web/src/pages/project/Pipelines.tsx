// CI pipelines and jobs (Build › Pipelines and Jobs), through the GitLab APIs: the pipeline
// list and page (stages and their jobs), the job list and page (log, artifacts and actions),
// the artifacts browser, and a button that gives a merge request a random pipeline through
// labrat's test endpoint.

import { useMemo, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, labrat } from '../../api/client';
import type { ArtifactEntry, Job, MergeRequest, Pipeline, PipelineDetail, Project } from '../../api/types';
import { Empty, ErrorBox, Loading, StatusIcon, TimeAgo, UserLink, errorText, useToast } from '../../components/ui';
import { useProject, useProjectCrumbs } from './ProjectRoutes';

const ACTIVE = ['created', 'pending', 'running', 'preparing', 'waiting_for_resource', 'canceling', 'scheduled'];

/** `12m 3s` for a duration in seconds. */
export function duration(s: number | null | undefined): string {
  if (s == null) return '';
  const t = Math.round(s);
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const sec = t % 60;
  return h ? `${h}h ${m}m ${sec}s` : m ? `${m}m ${sec}s` : `${sec}s`;
}

const formatSize = (n: number) => (n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KiB` : `${(n / 1024 / 1024).toFixed(1)} MiB`);

/** Stages in pipeline order (jobs come newest first; a stage's first job tells its place). */
export function stagesOf(jobs: Job[]): { name: string; jobs: Job[]; status: string }[] {
  const sorted = [...jobs].sort((a, b) => a.id - b.id);
  const out: { name: string; jobs: Job[]; status: string }[] = [];
  for (const j of sorted) {
    let s = out.find((x) => x.name === j.stage);
    if (!s) {
      s = { name: j.stage, jobs: [], status: 'created' };
      out.push(s);
    }
    s.jobs.push(j);
  }
  for (const s of out) s.status = stageStatus(s.jobs);
  return out;
}

/** A simplified `Gitlab::Ci::Status::Composite` for a stage's icon. */
function stageStatus(jobs: Job[]): string {
  const st = jobs.map((j) => (j.allow_failure && j.status === 'failed' ? 'success' : j.allow_failure && j.status === 'manual' ? 'skipped' : j.status));
  const only = (...a: string[]) => st.every((s) => a.includes(s));
  if (only('skipped')) return 'skipped';
  if (only('success', 'skipped')) return 'success';
  if (only('created')) return 'created';
  if (only('canceled', 'success', 'skipped')) return 'canceled';
  if (only('pending', 'created', 'skipped')) return 'pending';
  if (st.some((s) => s === 'running' || s === 'pending')) return 'running';
  if (st.includes('manual')) return 'manual';
  if (st.includes('created')) return 'running';
  return 'failed';
}

/** A pipeline's stages as small status icons. */
function MiniGraph({ projectId, pipelineId }: { projectId: number; pipelineId: number }) {
  const q = useQuery({ queryKey: ['pipeline-jobs', projectId, pipelineId], queryFn: () => api.all<Job>(`/projects/${projectId}/pipelines/${pipelineId}/jobs`, { per_page: 100 }) });
  if (!q.data) return null;
  return (
    <span style={{ display: 'inline-flex', gap: 4 }}>
      {stagesOf(q.data).map((s) => (
        <span key={s.name} title={`${s.name}: ${s.status}`}>
          <StatusIcon status={s.status} />
        </span>
      ))}
    </span>
  );
}

export function PipelinesList() {
  const { project } = useProject();
  useProjectCrumbs([{ label: 'Pipelines' }]);
  const [scope, setScope] = useState<'' | 'running' | 'finished'>('');
  const q = useQuery({
    queryKey: ['pipelines', project.id, scope],
    queryFn: () => api.get<Pipeline[]>(`/projects/${project.id}/pipelines`, scope ? { scope, per_page: 50 } : { per_page: 50 }),
    refetchInterval: 5000,
  });
  return (
    <>
      <div className="page-head">
        <h1>Pipelines</h1>
      </div>
      <div className="tabs">
        {([['', 'All'], ['running', 'Running'], ['finished', 'Finished']] as const).map(([s, label]) => (
          <button key={s} className={`tab${scope === s ? ' active' : ''}`} onClick={() => setScope(s)}>
            {label}
          </button>
        ))}
      </div>
      <ErrorBox error={q.error} />
      {q.isLoading ? (
        <Loading />
      ) : !q.data?.length ? (
        <Empty title="No pipelines">
          <p>labrat runs no CI. Pipelines come from labrat's test endpoints (Settings › labrat test setup), the fake CI, and commit statuses.</p>
        </Empty>
      ) : (
        <PipelineTable project={project} pipelines={q.data} />
      )}
    </>
  );
}

export function PipelineTable({ project, pipelines }: { project: Project; pipelines: Pipeline[] }) {
  return (
    <div className="table-wrap">
      <table className="table">
        <thead>
          <tr>
            <th>Status</th>
            <th>Pipeline</th>
            <th>Ref</th>
            <th>Commit</th>
            <th>Stages</th>
            <th>Created</th>
          </tr>
        </thead>
        <tbody>
          {pipelines.map((p) => (
            <tr key={p.id}>
              <td>
                <StatusIcon status={p.status} label />
              </td>
              <td>
                <Link to={`/${project.path_with_namespace}/-/pipelines/${p.id}`}>#{p.id}</Link>
                <div className="subtle" style={{ fontSize: 12 }}>
                  {p.source}
                </div>
              </td>
              <td className="mono">{p.ref}</td>
              <td>
                <Link to={`/${project.path_with_namespace}/-/commit/${p.sha}`}>
                  <code>{p.sha.slice(0, 8)}</code>
                </Link>
              </td>
              <td>
                <MiniGraph projectId={project.id} pipelineId={p.id} />
              </td>
              <td>
                <TimeAgo at={p.created_at} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Retry, cancel and play buttons for a job, as its status allows. */
function JobActions({ project, job, small }: { project: Project; job: Job; small?: boolean }) {
  const { canWrite } = useProject();
  const qc = useQueryClient();
  const toast = useToast();
  const act = useMutation({
    mutationFn: (action: string) => api.post<Job>(`/projects/${project.id}/jobs/${job.id}/${action}`),
    onSuccess: () => qc.invalidateQueries(),
    onError: (e) => toast(errorText(e), true),
  });
  if (!canWrite) return null;
  const cls = `btn ${small ? 'btn-tertiary btn-sm' : ''}`;
  const actions: [string, string][] = [];
  if (job.status === 'manual') actions.push(['play', 'Run']);
  if (['created', 'pending', 'running', 'preparing', 'scheduled', 'waiting_for_resource'].includes(job.status)) actions.push(['cancel', 'Cancel']);
  if (['success', 'failed', 'canceled', 'canceling'].includes(job.status)) actions.push(['retry', 'Run again']);
  return (
    <>
      {actions.map(([a, label]) => (
        <button key={a} className={cls} disabled={act.isPending} onClick={() => act.mutate(a)} title={label}>
          {label}
        </button>
      ))}
    </>
  );
}

export function PipelinePage({ id, tab }: { id: number; tab: string }) {
  const { project, canWrite, canMaintain } = useProject();
  useProjectCrumbs([{ label: 'Pipelines', to: `/${project.path_with_namespace}/-/pipelines` }, { label: `#${id}` }]);
  const qc = useQueryClient();
  const toast = useToast();
  const nav = useNavigate();
  const base = `/projects/${project.id}/pipelines/${id}`;
  const pl = useQuery({ queryKey: ['pipeline', project.id, id], queryFn: () => api.get<PipelineDetail>(base), refetchInterval: (q) => (q.state.data && !ACTIVE.includes(q.state.data.status) ? false : 3000) });
  const jobs = useQuery({
    queryKey: ['pipeline-jobs', project.id, id],
    queryFn: () => api.all<Job>(`${base}/jobs`, { per_page: 100 }),
    refetchInterval: 3000,
  });
  const act = useMutation({
    mutationFn: async (a: 'retry' | 'cancel' | 'delete') => (a === 'delete' ? api.del(base) : api.post(`${base}/${a}`)),
    onSuccess: (_, a) => {
      if (a === 'delete') nav(`/${project.path_with_namespace}/-/pipelines`);
      qc.invalidateQueries();
    },
    onError: (e) => toast(errorText(e), true),
  });
  if (pl.isLoading) return <Loading />;
  if (pl.error || !pl.data) return <ErrorBox error={pl.error ?? 'Not found'} />;
  const p = pl.data;
  const list = jobs.data ?? [];
  const stages = stagesOf(list);
  const retryable = list.some((j) => j.status === 'failed' || j.status === 'canceled');
  const cancelable = list.some((j) => ['created', 'pending', 'running', 'manual'].includes(j.status));
  const pp = `/${project.path_with_namespace}/-/pipelines/${id}`;
  return (
    <>
      <div className="page-head" style={{ alignItems: 'center' }}>
        <h1 style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <StatusIcon status={p.status} /> Pipeline #{p.id}
          {p.name && <span className="subtle"> · {p.name}</span>}
        </h1>
        <span className="spacer" />
        {canWrite && retryable && (
          <button className="btn" disabled={act.isPending} onClick={() => act.mutate('retry')}>
            Retry
          </button>
        )}
        {canWrite && cancelable && (
          <button className="btn btn-danger-secondary" disabled={act.isPending} onClick={() => act.mutate('cancel')}>
            Cancel pipeline
          </button>
        )}
        {canMaintain && (
          <button className="btn btn-danger-secondary" disabled={act.isPending} onClick={() => confirm('Delete this pipeline?') && act.mutate('delete')}>
            Delete
          </button>
        )}
      </div>
      <p className="subtle">
        {p.detailed_status.label} · <span className="mono">{p.ref}</span> ·{' '}
        <Link to={`/${project.path_with_namespace}/-/commit/${p.sha}`}>
          <code>{p.sha.slice(0, 8)}</code>
        </Link>{' '}
        · {p.source}
        {p.user && (
          <>
            {' '}
            · by <UserLink user={p.user} />
          </>
        )}{' '}
        · created <TimeAgo at={p.created_at} />
        {p.duration != null && <> · took {duration(p.duration)}</>}
        {p.coverage && <> · coverage {p.coverage}%</>}
      </p>
      <div className="tabs">
        <Link className={`tab${tab !== 'jobs' ? ' active' : ''}`} to={pp}>
          Pipeline
        </Link>
        <Link className={`tab${tab === 'jobs' ? ' active' : ''}`} to={`${pp}/jobs`}>
          Jobs <span className="count">{list.length}</span>
        </Link>
      </div>
      <ErrorBox error={jobs.error} />
      {tab === 'jobs' ? (
        <JobTable project={project} jobs={list} />
      ) : (
        <div style={{ display: 'flex', gap: 24, overflowX: 'auto', paddingBottom: 8 }}>
          {stages.map((s) => (
            <div key={s.name} style={{ minWidth: 200 }}>
              <h3 style={{ fontSize: 13, textTransform: 'capitalize', marginBottom: 8, display: 'flex', gap: 6, alignItems: 'center' }}>
                <StatusIcon status={s.status} /> {s.name}
              </h3>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                {s.jobs.map((j) => (
                  <div key={j.id} className="card" style={{ padding: '6px 8px', display: 'flex', alignItems: 'center', gap: 6, border: '1px solid var(--border)', borderRadius: 'var(--radius)' }}>
                    <StatusIcon status={j.status} />
                    <Link to={`/${project.path_with_namespace}/-/jobs/${j.id}`} style={{ flex: 1 }} title={`${j.name}: ${j.status}${j.allow_failure ? ' (allowed to fail)' : ''}`}>
                      {j.name}
                    </Link>
                    <JobActions project={project} job={j} small />
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
}

export function JobTable({ project, jobs }: { project: Project; jobs: Job[] }) {
  if (!jobs.length) return <Empty title="No jobs" />;
  return (
    <div className="table-wrap">
      <table className="table">
        <thead>
          <tr>
            <th>Status</th>
            <th>Job</th>
            <th>Stage</th>
            <th>Pipeline</th>
            <th>Duration</th>
            <th>Coverage</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {jobs.map((j) => (
            <tr key={j.id}>
              <td>
                <StatusIcon status={j.status} label />
              </td>
              <td>
                <Link to={`/${project.path_with_namespace}/-/jobs/${j.id}`}>{j.name}</Link>{' '}
                <span className="subtle">#{j.id}</span>
                {j.allow_failure && j.status === 'failed' && <div className="subtle" style={{ fontSize: 12 }}>allowed to fail</div>}
              </td>
              <td>{j.stage}</td>
              <td>
                <Link to={`/${project.path_with_namespace}/-/pipelines/${j.pipeline.id}`}>#{j.pipeline.id}</Link>
              </td>
              <td>{duration(j.duration)}</td>
              <td>{j.coverage != null ? `${j.coverage}%` : ''}</td>
              <td style={{ textAlign: 'right', whiteSpace: 'nowrap' }}>
                <JobActions project={project} job={j} small />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function JobsList() {
  const { project } = useProject();
  useProjectCrumbs([{ label: 'Jobs' }]);
  const q = useQuery({ queryKey: ['jobs', project.id], queryFn: () => api.get<Job[]>(`/projects/${project.id}/jobs`, { per_page: 50 }), refetchInterval: 5000 });
  return (
    <>
      <div className="page-head">
        <h1>Jobs</h1>
      </div>
      <ErrorBox error={q.error} />
      {q.isLoading ? <Loading /> : <JobTable project={project} jobs={q.data ?? []} />}
    </>
  );
}

const COLORS = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white'];
const CSS: Record<string, string> = { black: '#1f1e24', red: '#dd2b0e', green: '#108548', yellow: '#ab6100', blue: '#1f75cb', magenta: '#a3289e', cyan: '#0d8296', white: '#bfbfc3' };

/** A job log with its ANSI colors, without GitLab's section markers. */
function Log({ text }: { text: string }) {
  const parts = useMemo(() => {
    const clean = text.replace(/section_(start|end):\d+:[^\r\n]*\r?\x1b\[0K/g, '').replace(/\x1b\[0K/g, '');
    const out: { t: string; style: React.CSSProperties }[] = [];
    let style: React.CSSProperties = {};
    let last = 0;
    const re = /\x1b\[([0-9;]*)m/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(clean))) {
      if (m.index > last) out.push({ t: clean.slice(last, m.index), style });
      last = re.lastIndex;
      const next: React.CSSProperties = { ...style };
      for (const c of (m[1] || '0').split(';').map(Number)) {
        if (c === 0) Object.keys(next).forEach((k) => delete next[k as keyof React.CSSProperties]);
        else if (c === 1) next.fontWeight = 600;
        else if (c >= 30 && c <= 37) next.color = CSS[COLORS[c - 30]];
        else if (c >= 90 && c <= 97) next.color = CSS[COLORS[c - 90]];
        else if (c === 39) delete next.color;
      }
      style = next;
    }
    if (last < clean.length) out.push({ t: clean.slice(last), style });
    return out;
  }, [text]);
  return (
    <pre className="job-log" style={{ background: '#1f1e24', color: '#ececef', padding: 12, borderRadius: 'var(--radius)', overflow: 'auto', minHeight: 120, fontSize: 12.5, lineHeight: 1.5 }}>
      {parts.map((p, i) => (
        <span key={i} style={p.style.color === CSS.black ? { ...p.style, color: '#89888d' } : p.style}>
          {p.t}
        </span>
      ))}
    </pre>
  );
}

export function JobPage({ id }: { id: number }) {
  const { project, canWrite, canMaintain } = useProject();
  const qc = useQueryClient();
  const toast = useToast();
  const base = `/projects/${project.id}/jobs/${id}`;
  const q = useQuery({ queryKey: ['job', project.id, id], queryFn: () => api.get<Job>(base), refetchInterval: (q) => (q.state.data && !ACTIVE.includes(q.state.data.status) ? false : 3000) });
  const log = useQuery({ queryKey: ['job-log', project.id, id], queryFn: () => api.text(`${base}/trace`), refetchInterval: (q) => (q.state.data !== undefined && q.state.dataUpdatedAt && !ACTIVE.includes(qc.getQueryData<Job>(['job', project.id, id])?.status ?? '') ? false : 3000) });
  useProjectCrumbs([{ label: 'Jobs', to: `/${project.path_with_namespace}/-/jobs` }, { label: q.data ? `${q.data.name} #${id}` : `#${id}` }]);
  const act = useMutation({
    mutationFn: (a: string) => (a === 'delete-artifacts' ? api.del(`${base}/artifacts`) : api.post(`${base}/${a}`)),
    onSuccess: () => qc.invalidateQueries(),
    onError: (e) => toast(errorText(e), true),
  });
  if (q.isLoading) return <Loading />;
  if (q.error || !q.data) return <ErrorBox error={q.error ?? 'Not found'} />;
  const j = q.data;
  const api4 = `/api/v4/projects/${project.id}/jobs/${id}`;
  return (
    <div style={{ display: 'flex', gap: 24, alignItems: 'flex-start', flexWrap: 'wrap' }}>
      <div style={{ flex: '1 1 480px', minWidth: 0 }}>
        <div className="page-head" style={{ alignItems: 'center' }}>
          <h1 style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <StatusIcon status={j.status} /> {j.name}
          </h1>
          <span className="spacer" />
          <JobActions project={project} job={j} />
          {canWrite && j.status !== 'running' && (j.artifacts.length > 0 || log.data) && !j.erased_at && (
            <button className="btn btn-danger-secondary" onClick={() => confirm('Erase the job log and artifacts?') && act.mutate('erase')}>
              Erase
            </button>
          )}
        </div>
        {j.failure_reason && <div className="alert alert-danger">This job failed: {j.failure_reason.replace(/_/g, ' ')}.{j.allow_failure && ' It was allowed to fail.'}</div>}
        {j.erased_at ? <Empty title="Job has been erased" /> : log.data ? <Log text={log.data} /> : <Empty title={j.status === 'manual' ? 'This job requires a manual action' : 'This job does not have a log'} />}
      </div>
      <aside style={{ flex: '0 0 280px' }} className="subtle">
        <dl className="details" style={{ display: 'grid', gridTemplateColumns: 'auto 1fr', gap: '4px 12px', margin: 0 }}>
          <dt>Status</dt>
          <dd>{j.status}</dd>
          {j.duration != null && (
            <>
              <dt>Duration</dt>
              <dd>{duration(j.duration)}</dd>
            </>
          )}
          {j.queued_duration != null && (
            <>
              <dt>Queued</dt>
              <dd>{duration(j.queued_duration)}</dd>
            </>
          )}
          {j.finished_at && (
            <>
              <dt>Finished</dt>
              <dd>
                <TimeAgo at={j.finished_at} />
              </dd>
            </>
          )}
          {j.runner && (
            <>
              <dt>Runner</dt>
              <dd>{j.runner.description}</dd>
            </>
          )}
          {j.coverage != null && (
            <>
              <dt>Coverage</dt>
              <dd>{j.coverage}%</dd>
            </>
          )}
          <dt>Pipeline</dt>
          <dd>
            <Link to={`/${project.path_with_namespace}/-/pipelines/${j.pipeline.id}`}>#{j.pipeline.id}</Link> · {j.stage}
          </dd>
          <dt>Ref</dt>
          <dd className="mono">{j.ref}</dd>
          {j.tag_list.length > 0 && (
            <>
              <dt>Tags</dt>
              <dd>{j.tag_list.join(', ')}</dd>
            </>
          )}
        </dl>
        {j.artifacts_file && (
          <div style={{ marginTop: 16 }}>
            <h3 style={{ fontSize: 14 }}>Job artifacts</h3>
            <p style={{ fontSize: 12 }}>
              {j.artifacts_expire_at ? (
                <>
                  Expire <TimeAgo at={j.artifacts_expire_at} />
                </>
              ) : (
                'Kept forever'
              )}{' '}
              · {formatSize(j.artifacts_file.size)}
            </p>
            <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
              <a className="btn btn-sm" href={`${api4}/artifacts`}>
                Download
              </a>
              <Link className="btn btn-sm" to={`/${project.path_with_namespace}/-/jobs/${id}/artifacts/browse`}>
                Browse
              </Link>
              {canWrite && j.artifacts_expire_at && (
                <button className="btn btn-sm" onClick={() => act.mutate('artifacts/keep')}>
                  Keep
                </button>
              )}
              {canMaintain && (
                <button className="btn btn-sm btn-danger-secondary" onClick={() => confirm('Delete the artifacts?') && act.mutate('delete-artifacts')}>
                  Delete
                </button>
              )}
            </div>
          </div>
        )}
      </aside>
    </div>
  );
}

export function ArtifactsBrowser({ id, path }: { id: number; path: string }) {
  const { project } = useProject();
  const jp = `/${project.path_with_namespace}/-/jobs/${id}`;
  useProjectCrumbs([{ label: 'Jobs', to: `/${project.path_with_namespace}/-/jobs` }, { label: `#${id}`, to: jp }, { label: 'Artifacts' }]);
  const q = useQuery({ queryKey: ['artifacts-tree', project.id, id, path], queryFn: () => api.all<ArtifactEntry>(`/projects/${project.id}/jobs/${id}/artifacts/tree`, { path, per_page: 100 }) });
  const up = path.split('/').filter(Boolean).slice(0, -1).join('/');
  return (
    <>
      <div className="page-head">
        <h1>
          Artifacts <span className="subtle mono">/{path}</span>
        </h1>
      </div>
      <ErrorBox error={q.error} />
      {q.isLoading ? (
        <Loading />
      ) : (
        <div className="table-wrap">
          <table className="table">
            <tbody>
              {path && (
                <tr>
                  <td colSpan={2}>
                    <Link to={`${jp}/artifacts/browse${up ? `/${up}` : ''}`}>..</Link>
                  </td>
                </tr>
              )}
              {(q.data ?? []).map((e) => (
                <tr key={e.path}>
                  <td>
                    {e.type === 'directory' ? (
                      <Link to={`${jp}/artifacts/browse/${e.path.replace(/\/$/, '')}`}>{e.name}/</Link>
                    ) : (
                      <a href={`/api/v4/projects/${project.id}/jobs/${id}/artifacts/${e.path}`}>{e.name}</a>
                    )}
                  </td>
                  <td className="subtle">{e.size != null ? formatSize(e.size) : ''}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------------------------
// Random pipelines (labrat test endpoint).

const pick = <T,>(a: T[]): T => a[Math.floor(Math.random() * a.length)];
const rnd = (lo: number, hi: number) => lo + Math.floor(Math.random() * (hi - lo + 1));

/** A random pipeline run: stages up to a random point are done (passing, or one failing), that
 *  stage is in progress or finished, and the rest wait to be processed. */
export function randomRun(): Record<string, unknown> {
  const all = [
    { stage: 'build', names: ['compile', 'build', 'assets'] },
    { stage: 'test', names: ['unit', 'integration', 'lint', 'e2e'] },
    { stage: 'deploy', names: ['staging', 'review'] },
  ];
  const stages = all.slice(0, rnd(1, 3));
  const at = rnd(0, stages.length - 1);
  const atOutcome = pick(['running', 'running', 'pending', 'success', 'failed']);
  const jobs: Record<string, unknown>[] = [];
  stages.forEach((s, i) => {
    const names = [...s.names].sort(() => Math.random() - 0.5).slice(0, rnd(1, Math.min(3, s.names.length)));
    names.forEach((name, k) => {
      const job: Record<string, unknown> = { name, stage: s.stage };
      if (s.stage === 'deploy' && k === names.length - 1 && Math.random() < 0.5) job.when = 'manual';
      if (name === 'lint') job.allow_failure = true;
      let status: string | undefined;
      if (i < at) status = name === 'lint' && Math.random() < 0.5 ? 'failed' : 'success';
      else if (i === at && job.when !== 'manual') status = k === 0 ? atOutcome : pick(['success', atOutcome]);
      if (status) {
        job.status = status;
        job.duration = rnd(3, 240);
        job.queued_duration = rnd(0, 20);
        const lines = [`\u001b[32;1mRunning ${name}\u001b[0;m`, `$ echo "${s.stage}: ${name}"`, `${s.stage}: ${name}`];
        if (status === 'failed') lines.push('\u001b[31;1mERROR: Job failed: exit code 1\u001b[0;m');
        if (status === 'success') lines.push('\u001b[32;1mJob succeeded\u001b[0;m');
        if (name === 'unit' && status === 'success') {
          const cov = (rnd(500, 990) / 10).toFixed(1);
          lines.splice(3, 0, `Coverage: ${cov}%`);
          job.coverage = Number(cov);
        }
        job.log = lines.join('\n') + '\n';
        if (status === 'failed') job.failure_reason = 'script_failure';
        if (s.stage === 'build' && status === 'success') {
          job.artifacts = [{ file_type: 'archive', files: { [`dist/${name}.txt`]: `${name} output\n`, 'dist/VERSION': '1.0.0\n' }, expire_in: '1 week' }];
        }
      }
      jobs.push(job);
    });
  });
  return { jobs };
}

/** Gives the MR a random pipeline for its head (a merge request pipeline). */
export function RandomPipelineButton({ project, mr, className }: { project: Project; mr: MergeRequest; className?: string }) {
  const qc = useQueryClient();
  const toast = useToast();
  const m = useMutation({
    mutationFn: () => labrat('POST', `/projects/${project.id}/pipelines`, { ...randomRun(), merge_request_iid: mr.iid }),
    onSuccess: () => {
      toast('Pipeline created');
      qc.invalidateQueries();
    },
    onError: (e) => toast(errorText(e), true),
  });
  return (
    <button className={className ?? 'btn btn-sm'} disabled={m.isPending} onClick={() => m.mutate()} title="Create a random pipeline for this merge request's head through labrat's test endpoint">
      Randomize pipeline
    </button>
  );
}
