import { useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Commit, FileDiff, MergeRequest } from '../../api/types';
import { Avatar, CiStatus, CopyButton, Empty, ErrorBox, Loading, Pager, TimeAgo } from '../../components/ui';
import { useProject, useProjectCrumbs } from './ProjectRoutes';
import { RefSelector, splitRef, useRefs } from './refs';
import { DiffFile } from '../mr/DiffView';
import { encPath, localPath } from '../../lib/paths';

export function CommitList({ commits, projectPath }: { commits: Commit[]; projectPath: string }) {
  // Grouped by day, as GitLab does.
  const groups: [string, Commit[]][] = [];
  for (const c of commits) {
    const day = new Date(c.committed_date).toLocaleDateString(undefined, { dateStyle: 'medium' });
    const last = groups[groups.length - 1];
    if (last && last[0] === day) last[1].push(c);
    else groups.push([day, [c]]);
  }
  return (
    <>
      {groups.map(([day, list]) => (
        <div key={day} style={{ marginBottom: 16 }}>
          <div className="small subtle" style={{ margin: '0 0 6px', fontWeight: 600 }}>
            {day} <span style={{ fontWeight: 400 }}>{list.length} commit{list.length === 1 ? '' : 's'}</span>
          </div>
          <ul className="list list-bordered">
            {list.map((c) => (
              <li key={c.id} className="commit-row" style={{ alignItems: 'center' }}>
                <Avatar name={c.author_name} size={32} />
                <div className="grow">
                  <Link to={`/${projectPath}/-/commit/${c.id}`} className="title">
                    {c.title}
                  </Link>
                  <div className="meta">
                    {c.author_name} authored <TimeAgo at={c.authored_date} />
                  </div>
                </div>
                <CiStatus pipeline={c.last_pipeline} />
                <code className="sha">{c.short_id}</code>
                <CopyButton text={c.id} title="Copy commit SHA" />
              </li>
            ))}
          </ul>
        </div>
      ))}
    </>
  );
}

export function Commits({ parts }: { parts: string[] }) {
  const { project } = useProject();
  const refs = useRefs(project.id);
  const nav = useNavigate();
  const names = refs.data ? [...refs.data.branches.map((b) => b.name), ...refs.data.tags.map((t) => t.name)] : undefined;
  const [ref_, path] = splitRef(parts, names, project.default_branch ?? 'main');
  const [page, setPage] = useState(1);
  useProjectCrumbs([{ label: 'Commits' }]);
  const q = useQuery({
    queryKey: ['commits', project.id, ref_, path, page],
    queryFn: () => api.page<Commit>(`/projects/${project.id}/repository/commits`, { ref_name: ref_, path: path || undefined, page, per_page: 40 }),
    enabled: !!names || parts.length <= 1,
  });
  return (
    <>
      <div className="page-head">
        <h1>Commits</h1>
      </div>
      <div className="repo-head">
        <RefSelector projectId={project.id} value={ref_} onSelect={(r) => nav(`/${project.path_with_namespace}/-/commits/${r}${path ? `/${encPath(path)}` : ''}`)} />
        {path && <code>{path}</code>}
      </div>
      <ErrorBox error={q.error} />
      {q.isLoading ? (
        <Loading />
      ) : q.data?.items.length ? (
        <>
          <CommitList commits={q.data.items} projectPath={project.path_with_namespace} />
          <Pager page={q.data.page} totalPages={q.data.totalPages} nextPage={q.data.nextPage} onPage={setPage} />
        </>
      ) : (
        <Empty title="No commits" />
      )}
    </>
  );
}

export function CommitPage({ sha }: { sha: string }) {
  const { project } = useProject();
  useProjectCrumbs([{ label: 'Commits', to: `/${project.path_with_namespace}/-/commits` }, { label: sha.slice(0, 8) }]);
  const c = useQuery({ queryKey: ['commit', project.id, sha], queryFn: () => api.get<Commit>(`/projects/${project.id}/repository/commits/${sha}`) });
  const diff = useQuery({ queryKey: ['commit-diff', project.id, sha], queryFn: () => api.all<FileDiff>(`/projects/${project.id}/repository/commits/${sha}/diff`) });
  const mrs = useQuery({ queryKey: ['commit-mrs', project.id, sha], queryFn: () => api.get<MergeRequest[]>(`/projects/${project.id}/repository/commits/${sha}/merge_requests`) });
  if (c.isLoading) return <Loading />;
  if (c.error) return <ErrorBox error={c.error} />;
  const commit = c.data!;
  const body = commit.message.slice(commit.title.length).trim();
  return (
    <>
      <div className="page-head">
        <div className="grow">
          <div className="row small subtle">
            Commit <code>{commit.short_id}</code> <CopyButton text={commit.id} /> authored <TimeAgo at={commit.authored_date} /> by <strong style={{ color: 'var(--text)' }}>{commit.author_name}</strong>
          </div>
        </div>
        <div className="actions">
          <Link className="btn" to={`/${project.path_with_namespace}/-/tree/${commit.id}`}>
            Browse files
          </Link>
        </div>
      </div>
      <h2 style={{ marginBottom: 8 }}>{commit.title}</h2>
      {body && <pre style={{ whiteSpace: 'pre-wrap', fontFamily: 'var(--font)', margin: '0 0 12px' }}>{body}</pre>}
      <div className="small subtle" style={{ marginBottom: 16 }}>
        {commit.parent_ids.length ? (
          <>
            {commit.parent_ids.length} parent{commit.parent_ids.length > 1 ? 's' : ''}{' '}
            {commit.parent_ids.map((p) => (
              <Link key={p} to={`/${project.path_with_namespace}/-/commit/${p}`} style={{ marginRight: 6 }}>
                <code>{p.slice(0, 8)}</code>
              </Link>
            ))}
          </>
        ) : (
          'Root commit'
        )}
        {mrs.data?.map((mr) => (
          <span key={mr.id}>
            {' '}
            · in merge request <Link to={localPath(mr.web_url)}>{mr.references.full}</Link>
          </span>
        ))}
      </div>
      {diff.isLoading ? <Loading /> : diff.data?.map((f) => <DiffFile key={f.new_path + f.old_path} file={f} />)}
    </>
  );
}
