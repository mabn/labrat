import { useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import type { Group } from '../api/types';
import { Layout, useCrumbs } from '../components/Layout';
import { ErrorBox } from '../components/ui';

const slug = (s: string) =>
  s
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_.-]+/g, '-')
    .replace(/^[-.]+|[-.]+$/g, '');

export function NewGroup() {
  const nav = useNavigate();
  const [params] = useSearchParams();
  useCrumbs([{ label: 'Your work', to: '/' }, { label: 'Groups', to: '/dashboard/groups' }, { label: 'New group' }]);
  const groups = useQuery({ queryKey: ['groups', 'owned'], queryFn: () => api.get<Group[]>('/groups', { min_access_level: 40, per_page: 100 }) });
  const [name, setName] = useState('');
  const [path, setPath] = useState('');
  const [edited, setEdited] = useState(false);
  const [parent, setParent] = useState(params.get('parent_id') ?? '');
  const [visibility, setVisibility] = useState('private');
  const [description, setDescription] = useState('');
  const create = useMutation({
    mutationFn: () =>
      api.post<Group>('/groups', {
        name,
        path: path || slug(name),
        parent_id: parent ? Number(parent) : undefined,
        visibility,
        description: description || undefined,
      }),
    onSuccess: (g) => nav(`/${g.full_path}`),
  });
  return (
    <Layout>
      <div className="page-head">
        <h1>Create group</h1>
      </div>
      <form
        className="form-narrow"
        onSubmit={(e) => {
          e.preventDefault();
          create.mutate();
        }}
      >
        <ErrorBox error={create.error} />
        <label className="field">
          <span className="label">Group name</span>
          <input
            type="text"
            autoFocus
            required
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              if (!edited) setPath(slug(e.target.value));
            }}
          />
        </label>
        <div className="row" style={{ alignItems: 'flex-start' }}>
          <label className="field grow">
            <span className="label">Parent group</span>
            <select value={parent} onChange={(e) => setParent(e.target.value)}>
              <option value="">None (top-level group)</option>
              {groups.data?.map((g) => (
                <option key={g.id} value={g.id}>
                  {g.full_path}
                </option>
              ))}
            </select>
          </label>
          <label className="field grow">
            <span className="label">Group URL</span>
            <input
              type="text"
              required
              value={path}
              onChange={(e) => {
                setPath(e.target.value);
                setEdited(true);
              }}
            />
          </label>
        </div>
        <label className="field">
          <span className="label">Description (optional)</span>
          <textarea rows={2} value={description} onChange={(e) => setDescription(e.target.value)} />
        </label>
        <div className="field">
          <span className="label-text">Visibility level</span>
          {['private', 'internal', 'public'].map((v) => (
            <label key={v} className="check">
              <input type="radio" checked={visibility === v} onChange={() => setVisibility(v)} />
              <span style={{ textTransform: 'capitalize' }}>{v}</span>
            </label>
          ))}
        </div>
        <div className="row">
          <button className="btn btn-confirm" disabled={create.isPending || !name}>
            Create group
          </button>
          <button type="button" className="btn" onClick={() => nav(-1)}>
            Cancel
          </button>
        </div>
      </form>
    </Layout>
  );
}
