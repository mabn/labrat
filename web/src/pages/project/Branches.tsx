import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api, enc } from '../../api/client';
import { Empty, ErrorBox, Loading, Modal, TimeAgo, useToast, errorText } from '../../components/ui';
import { Icon } from '../../components/Icon';
import { useProject, useProjectCrumbs } from './ProjectRoutes';
import { useRefs } from './refs';

function NewRefModal({ kind, onClose }: { kind: 'branch' | 'tag'; onClose: () => void }) {
  const { project } = useProject();
  const qc = useQueryClient();
  const [name, setName] = useState('');
  const [ref_, setRef] = useState(project.default_branch ?? 'main');
  const [message, setMessage] = useState('');
  const create = useMutation({
    mutationFn: () =>
      kind === 'branch'
        ? api.post(`/projects/${project.id}/repository/branches`, { branch: name, ref: ref_ })
        : api.post(`/projects/${project.id}/repository/tags`, { tag_name: name, ref: ref_, message: message || undefined }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['refs', project.id] });
      onClose();
    },
  });
  return (
    <Modal
      title={kind === 'branch' ? 'New branch' : 'New tag'}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-confirm" disabled={!name || create.isPending} onClick={() => create.mutate()}>
            Create {kind}
          </button>
        </>
      }
    >
      <ErrorBox error={create.error} />
      <label className="field">
        <span className="label">{kind === 'branch' ? 'Branch name' : 'Tag name'}</span>
        <input type="text" autoFocus value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="field">
        <span className="label">Create from</span>
        <input type="text" value={ref_} onChange={(e) => setRef(e.target.value)} />
        <div className="help">Existing branch name, tag, or commit SHA</div>
      </label>
      {kind === 'tag' && (
        <label className="field">
          <span className="label">Message</span>
          <textarea rows={3} value={message} onChange={(e) => setMessage(e.target.value)} />
        </label>
      )}
    </Modal>
  );
}

export function Branches() {
  const { project, canWrite } = useProject();
  useProjectCrumbs([{ label: 'Branches' }]);
  const refs = useRefs(project.id);
  const qc = useQueryClient();
  const toast = useToast();
  const [creating, setCreating] = useState(false);
  const [search, setSearch] = useState('');
  const del = useMutation({
    mutationFn: (name: string) => api.del(`/projects/${project.id}/repository/branches/${enc(name)}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['refs', project.id] }),
    onError: (e) => toast(errorText(e), true),
  });
  const p = `/${project.path_with_namespace}/-`;
  const list = (refs.data?.branches ?? [])
    .filter((b) => b.name.toLowerCase().includes(search.toLowerCase()))
    .sort((a, b) => (a.default ? -1 : b.default ? 1 : b.commit.committed_date.localeCompare(a.commit.committed_date)));
  return (
    <>
      <div className="page-head">
        <h1>Branches</h1>
        <div className="actions">
          <input type="search" placeholder="Filter by branch name" value={search} onChange={(e) => setSearch(e.target.value)} style={{ width: 220 }} />
          {canWrite && (
            <button className="btn btn-confirm" onClick={() => setCreating(true)}>
              New branch
            </button>
          )}
        </div>
      </div>
      <ErrorBox error={refs.error} />
      {refs.isLoading ? (
        <Loading />
      ) : list.length ? (
        <ul className="list list-bordered">
          {list.map((b) => (
            <li key={b.name} style={{ alignItems: 'center' }}>
              <Icon name="branch" />
              <div className="grow">
                <div className="row">
                  <Link to={`${p}/tree/${b.name}`} className="title mono">
                    {b.name}
                  </Link>
                  {b.default && <span className="badge badge-info">default</span>}
                  {b.protected && <span className="badge badge-success">protected</span>}
                  {b.merged && <span className="badge badge-info">merged</span>}
                </div>
                <div className="meta">
                  <code>{b.commit.short_id}</code> · {b.commit.title} · <TimeAgo at={b.commit.committed_date} />
                </div>
              </div>
              {!b.default && (
                <Link className="btn btn-sm" to={`${p}/merge_requests/new?source_branch=${encodeURIComponent(b.name)}`}>
                  Merge request
                </Link>
              )}
              <Link className="btn btn-sm" to={`${p}/commits/${b.name}`}>
                Commits
              </Link>
              {canWrite && !b.default && (
                <button className="btn btn-sm btn-icon btn-danger-secondary" title="Delete branch" onClick={() => confirm(`Delete branch ${b.name}?`) && del.mutate(b.name)}>
                  <Icon name="trash" />
                </button>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <Empty title="No branches to show" />
      )}
      {creating && <NewRefModal kind="branch" onClose={() => setCreating(false)} />}
    </>
  );
}

export function Tags() {
  const { project, canWrite, canMaintain } = useProject();
  useProjectCrumbs([{ label: 'Tags' }]);
  const refs = useRefs(project.id);
  const qc = useQueryClient();
  const toast = useToast();
  const [creating, setCreating] = useState(false);
  const del = useMutation({
    mutationFn: (name: string) => api.del(`/projects/${project.id}/repository/tags/${enc(name)}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['refs', project.id] }),
    onError: (e) => toast(errorText(e), true),
  });
  return (
    <>
      <div className="page-head">
        <h1>Tags</h1>
        <div className="actions">
          {canWrite && (
            <button className="btn btn-confirm" onClick={() => setCreating(true)}>
              New tag
            </button>
          )}
        </div>
      </div>
      {refs.isLoading ? (
        <Loading />
      ) : refs.data?.tags.length ? (
        <ul className="list list-bordered">
          {refs.data.tags.map((t) => (
            <li key={t.name} style={{ alignItems: 'center' }}>
              <Icon name="tag" />
              <div className="grow">
                <Link className="title mono" to={`/${project.path_with_namespace}/-/tree/${t.name}`}>
                  {t.name}
                </Link>{' '}
                {t.protected && <span className="badge badge-success">protected</span>}
                <div className="meta">
                  <code>{t.commit.short_id}</code> · {t.commit.title}
                  {t.message && ` · ${t.message}`}
                </div>
              </div>
              {canMaintain && (
                <button className="btn btn-sm btn-icon btn-danger-secondary" title="Delete tag" onClick={() => confirm(`Delete tag ${t.name}?`) && del.mutate(t.name)}>
                  <Icon name="trash" />
                </button>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <Empty title="No tags" />
      )}
      {creating && <NewRefModal kind="tag" onClose={() => setCreating(false)} />}
    </>
  );
}
