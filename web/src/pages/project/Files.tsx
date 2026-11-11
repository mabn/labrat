import { useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, enc } from '../../api/client';
import type { Commit, Project, RepoFile, TreeEntry } from '../../api/types';
import { CopyButton, Empty, ErrorBox, Loading, Markdown, TimeAgo, useToast } from '../../components/ui';
import { Icon } from '../../components/Icon';
import { useProject, useProjectCrumbs } from './ProjectRoutes';
import { decodeContent, RefSelector, splitRef, useRefs } from './refs';
import { encPath } from '../../lib/paths';

export function LastCommit({ project, ref_, path }: { project: Project; ref_: string; path: string }) {
  const q = useQuery({
    queryKey: ['last-commit', project.id, ref_, path],
    queryFn: async () => (await api.get<Commit[]>(`/projects/${project.id}/repository/commits`, { ref_name: ref_, path: path || undefined, per_page: 1 }))[0] ?? null,
  });
  if (!q.data) return null;
  const c = q.data;
  return (
    <div className="card" style={{ marginBottom: 12 }}>
      <div className="card-body row" style={{ padding: '10px 16px' }}>
        <div className="grow">
          <Link to={`/${project.path_with_namespace}/-/commit/${c.id}`} style={{ color: 'var(--text)', fontWeight: 600 }}>
            {c.title}
          </Link>
          <div className="small subtle">
            {c.author_name} authored <TimeAgo at={c.authored_date} />
          </div>
        </div>
        <code className="small">{c.short_id}</code>
        <CopyButton text={c.id} title="Copy commit SHA" />
      </div>
    </div>
  );
}

export function TreeView({ project, ref_, path }: { project: Project; ref_: string; path: string }) {
  const q = useQuery({
    queryKey: ['tree', project.id, ref_, path],
    queryFn: () => api.all<TreeEntry>(`/projects/${project.id}/repository/tree`, { ref: ref_, path: path || undefined }, 10),
  });
  const p = `/${project.path_with_namespace}/-`;
  if (q.isLoading) return <Loading />;
  if (q.error) return <ErrorBox error={q.error} />;
  const entries = [...(q.data ?? [])].sort((a, b) => (a.type === b.type ? a.name.localeCompare(b.name) : a.type === 'tree' ? -1 : 1));
  const parent = path.split('/').slice(0, -1).join('/');
  return (
    <div className="table-wrap">
      <table className="table tree-table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Mode</th>
          </tr>
        </thead>
        <tbody>
          {path && (
            <tr>
              <td className="name" colSpan={2}>
                <Link to={`${p}/tree/${ref_}${parent ? `/${encPath(parent)}` : ''}`}>..</Link>
              </td>
            </tr>
          )}
          {entries.map((e) => (
            <tr key={e.path}>
              <td className="name">
                <Link to={`${p}/${e.type === 'tree' ? 'tree' : 'blob'}/${ref_}/${encPath(e.path)}`}>
                  <Icon name={e.type === 'tree' ? 'folder' : e.type === 'commit' ? 'commit' : 'file'} /> {e.name}
                </Link>
              </td>
              <td className="small subtle mono">{e.mode}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function PathCrumbs({ project, ref_, path }: { project: Project; ref_: string; path: string }) {
  const segs = path ? path.split('/') : [];
  const base = `/${project.path_with_namespace}/-/tree/${ref_}`;
  return (
    <span className="row" style={{ gap: 4, fontWeight: 600 }}>
      <Link to={base}>{project.path}</Link>
      {segs.map((s, i) => (
        <span key={i} className="row" style={{ gap: 4 }}>
          <span className="subtle">/</span>
          <Link to={`${base}/${encPath(segs.slice(0, i + 1).join('/'))}`}>{s}</Link>
        </span>
      ))}
    </span>
  );
}

export function EmptyRepo({ project }: { project: Project }) {
  const { canWrite } = useProject();
  const branch = project.default_branch || 'main';
  return (
    <div className="card">
      <div className="card-head">The repository for this project is empty</div>
      <div className="card-body">
        {canWrite && (
          <p>
            <Link className="btn btn-confirm" to={`/${project.path_with_namespace}/-/new/${branch}`}>
              <Icon name="plus" /> New file
            </Link>
          </p>
        )}
        <h3 style={{ margin: '12px 0 8px' }}>Push an existing folder</h3>
        <pre className="code" style={{ background: 'var(--gray-10)', padding: 12, border: '1px solid var(--border)', borderRadius: 4 }}>
          {`cd existing_folder
git init --initial-branch=${branch}
git remote add origin ${project.http_url_to_repo}
git add .
git commit -m "Initial commit"
git push --set-upstream origin ${branch}`}
        </pre>
      </div>
    </div>
  );
}

export function Tree({ parts }: { parts: string[] }) {
  const { project, canWrite } = useProject();
  const refs = useRefs(project.id);
  const nav = useNavigate();
  const names = refs.data ? [...refs.data.branches.map((b) => b.name), ...refs.data.tags.map((t) => t.name)] : undefined;
  const [ref_, path] = splitRef(parts, names, project.default_branch ?? 'main');
  useProjectCrumbs([{ label: 'Repository' }]);
  if (project.empty_repo) return <EmptyRepo project={project} />;
  if (parts.length > 1 && refs.isLoading) return <Loading />;
  const p = `/${project.path_with_namespace}/-`;
  return (
    <>
      <div className="repo-head" style={{ marginTop: 8 }}>
        <RefSelector projectId={project.id} value={ref_} onSelect={(r) => nav(`${p}/tree/${r}${path ? `/${encPath(path)}` : ''}`)} />
        <PathCrumbs project={project} ref_={ref_} path={path} />
        <span className="spacer" />
        <Link className="btn" to={`${p}/commits/${ref_}${path ? `/${encPath(path)}` : ''}`}>
          History
        </Link>
        {canWrite && (
          <Link className="btn" to={`${p}/new/${ref_}${path ? `/${encPath(path)}` : ''}`}>
            <Icon name="plus" /> New file
          </Link>
        )}
      </div>
      <LastCommit project={project} ref_={ref_} path={path} />
      <TreeView project={project} ref_={ref_} path={path} />
    </>
  );
}

export function Blob({ parts }: { parts: string[] }) {
  const { project, canWrite } = useProject();
  const refs = useRefs(project.id);
  const nav = useNavigate();
  const qc = useQueryClient();
  const toast = useToast();
  const names = refs.data ? [...refs.data.branches.map((b) => b.name), ...refs.data.tags.map((t) => t.name)] : undefined;
  const [ref_, path] = splitRef(parts, names, project.default_branch ?? 'main');
  useProjectCrumbs([{ label: 'Repository', to: `/${project.path_with_namespace}/-/tree/${ref_}` }, { label: path.split('/').pop() ?? '' }]);
  const q = useQuery({
    queryKey: ['file', project.id, ref_, path],
    queryFn: () => api.get<RepoFile>(`/projects/${project.id}/repository/files/${enc(path)}`, { ref: ref_ }),
    enabled: !!names,
  });
  const [rendered, setRendered] = useState(true);
  const del = useMutation({
    mutationFn: () => api.del(`/projects/${project.id}/repository/files/${enc(path)}`, { branch: ref_, commit_message: `Delete ${path}` }),
    onSuccess: () => {
      toast('File deleted');
      qc.invalidateQueries({ queryKey: ['tree', project.id] });
      nav(`/${project.path_with_namespace}/-/tree/${ref_}`);
    },
    onError: (e) => toast(String((e as Error).message), true),
  });
  const p = `/${project.path_with_namespace}/-`;
  if (!names || q.isLoading) return <Loading />;
  if (q.error) return <ErrorBox error={q.error} />;
  const f = q.data!;
  const { text, binary } = decodeContent(f.content);
  const isMd = /\.(md|markdown)$/i.test(path);
  const isBranch = refs.data?.branches.some((b) => b.name === ref_);
  const lines = text.endsWith('\n') ? text.slice(0, -1).split('\n') : text.split('\n');
  return (
    <>
      <div className="repo-head" style={{ marginTop: 8 }}>
        <RefSelector projectId={project.id} value={ref_} onSelect={(r) => nav(`${p}/blob/${r}/${encPath(path)}`)} />
        <PathCrumbs project={project} ref_={ref_} path={path} />
      </div>
      <LastCommit project={project} ref_={ref_} path={path} />
      <div className="blob-view">
        <div className="blob-head">
          <Icon name="file" />
          <strong>{f.file_name}</strong>
          <span className="small subtle">{f.size} bytes</span>
          <span className="spacer" />
          {isMd && (
            <div className="btn-group">
              <button className={`btn btn-sm${rendered ? ' btn-confirm' : ''}`} onClick={() => setRendered(true)}>
                Preview
              </button>
              <button className={`btn btn-sm${!rendered ? ' btn-confirm' : ''}`} onClick={() => setRendered(false)}>
                Code
              </button>
            </div>
          )}
          <CopyButton text={text} title="Copy file contents" />
          {canWrite && isBranch && (
            <>
              <Link className="btn btn-sm" to={`${p}/edit/${ref_}/${encPath(path)}`}>
                Edit
              </Link>
              <button className="btn btn-sm btn-danger-secondary" onClick={() => confirm(`Delete ${path} on ${ref_}?`) && del.mutate()}>
                Delete
              </button>
            </>
          )}
        </div>
        {binary ? (
          <Empty title="This file is binary and can’t be shown." />
        ) : isMd && rendered ? (
          <div style={{ padding: 24 }}>
            <Markdown source={text} projectPath={project.path_with_namespace} />
          </div>
        ) : (
          <div className="blob-code">
            <div className="nums">{lines.map((_, i) => i + 1).join('\n')}</div>
            <pre>{text}</pre>
          </div>
        )}
      </div>
    </>
  );
}

/** Creates or edits a file and commits it to the current or a new branch. */
export function EditFile({ parts, mode }: { parts: string[]; mode: 'new' | 'edit' }) {
  const { project } = useProject();
  const refs = useRefs(project.id);
  const nav = useNavigate();
  const qc = useQueryClient();
  const names = refs.data ? refs.data.branches.map((b) => b.name) : undefined;
  const [ref_, path] = splitRef(parts, names, project.default_branch ?? 'main');
  useProjectCrumbs([{ label: 'Repository', to: `/${project.path_with_namespace}/-/tree/${ref_}` }, { label: mode === 'new' ? 'New file' : 'Edit' }]);
  const existing = useQuery({
    queryKey: ['file', project.id, ref_, path],
    queryFn: () => api.get<RepoFile>(`/projects/${project.id}/repository/files/${enc(path)}`, { ref: ref_ }),
    enabled: mode === 'edit' && !!names,
  });
  const [fileName, setFileName] = useState(mode === 'new' ? (path ? `${path}/` : '') : '');
  const [content, setContent] = useState<string | null>(null);
  const [message, setMessage] = useState('');
  const [newBranch, setNewBranch] = useState(false);
  const [branch, setBranch] = useState(`patch-${Math.floor(Math.random() * 90000) + 10000}`);
  const [startMr, setStartMr] = useState(true);
  const original = existing.data ? decodeContent(existing.data.content).text : '';
  const body = content ?? (mode === 'edit' ? original : '');
  const target = mode === 'new' ? fileName : path;
  const empty = !!project.empty_repo;
  const commit = useMutation({
    mutationFn: () => {
      const payload = {
        branch: newBranch ? branch : ref_,
        start_branch: newBranch && !empty ? ref_ : undefined,
        content: body,
        commit_message: message || (mode === 'new' ? `Add new file` : `Update ${path.split('/').pop()}`),
        last_commit_id: mode === 'edit' ? existing.data?.last_commit_id : undefined,
      };
      const url = `/projects/${project.id}/repository/files/${enc(target)}`;
      return mode === 'new' ? api.post(url, payload) : api.put(url, payload);
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['tree', project.id] });
      qc.invalidateQueries({ queryKey: ['file', project.id] });
      qc.invalidateQueries({ queryKey: ['refs', project.id] });
      qc.invalidateQueries({ queryKey: ['project', project.id] });
      const p = `/${project.path_with_namespace}/-`;
      if (newBranch && startMr) nav(`${p}/merge_requests/new?source_branch=${encodeURIComponent(branch)}&target_branch=${encodeURIComponent(ref_)}`);
      else nav(`${p}/blob/${newBranch ? branch : ref_}/${encPath(target)}`);
    },
  });
  if (!names || (mode === 'edit' && existing.isLoading)) return <Loading />;
  return (
    <>
      <div className="page-head">
        <h1>{mode === 'new' ? 'New file' : `Edit ${path}`}</h1>
      </div>
      <ErrorBox error={existing.error || commit.error} />
      <div className="blob-view" style={{ marginBottom: 16 }}>
        <div className="blob-head">
          {mode === 'new' ? (
            <div className="input-group" style={{ width: 480 }}>
              <span>{ref_} /</span>
              <input type="text" placeholder="Filename" autoFocus value={fileName} onChange={(e) => setFileName(e.target.value)} />
            </div>
          ) : (
            <strong className="mono">{path}</strong>
          )}
        </div>
        <textarea
          value={body}
          onChange={(e) => setContent(e.target.value)}
          spellCheck={false}
          style={{ border: 0, borderRadius: 0, minHeight: 420, fontFamily: 'var(--mono)', fontSize: 13, boxShadow: 'none' }}
        />
      </div>
      <div className="form-narrow">
        <label className="field">
          <span className="label">Commit message</span>
          <textarea rows={3} value={message} placeholder={mode === 'new' ? 'Add new file' : `Update ${path.split('/').pop()}`} onChange={(e) => setMessage(e.target.value)} />
        </label>
        {!empty && (
          <div className="field">
            <span className="label-text">Branch</span>
            <label className="check">
              <input type="radio" checked={!newBranch} onChange={() => setNewBranch(false)} /> Commit to the current <code>{ref_}</code> branch
            </label>
            <label className="check">
              <input type="radio" checked={newBranch} onChange={() => setNewBranch(true)} /> Commit to a new branch
            </label>
            {newBranch && (
              <div style={{ paddingLeft: 24 }}>
                <input type="text" value={branch} onChange={(e) => setBranch(e.target.value)} style={{ marginBottom: 8 }} />
                <label className="check">
                  <input type="checkbox" checked={startMr} onChange={(e) => setStartMr(e.target.checked)} /> Create a merge request for this change
                </label>
              </div>
            )}
          </div>
        )}
        <div className="row">
          <button className="btn btn-confirm" disabled={commit.isPending || !target || target.endsWith('/')} onClick={() => commit.mutate()}>
            Commit changes
          </button>
          <button className="btn" onClick={() => nav(-1)}>
            Cancel
          </button>
        </div>
      </div>
    </>
  );
}
