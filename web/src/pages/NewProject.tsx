import { useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import type { Namespace, Project } from '../api/types';
import { Layout, useCrumbs } from '../components/Layout';
import { ErrorBox } from '../components/ui';
import { useSession } from '../session';
import { localPath } from '../lib/paths';

const slug = (s: string) =>
  s
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_.-]+/g, '-')
    .replace(/^[-.]+|[-.]+$/g, '');

export function NewProject() {
  const { user } = useSession();
  const nav = useNavigate();
  const [params] = useSearchParams();
  useCrumbs([{ label: 'Your work', to: '/' }, { label: 'Projects', to: '/dashboard/projects' }, { label: 'New project' }]);
  const namespaces = useQuery({ queryKey: ['namespaces'], queryFn: () => api.get<Namespace[]>('/namespaces', { per_page: 100 }) });
  const [name, setName] = useState('');
  const [path, setPath] = useState('');
  const [pathEdited, setPathEdited] = useState(false);
  const [namespaceId, setNamespaceId] = useState<string>(params.get('namespace_id') ?? '');
  const [description, setDescription] = useState('');
  const [visibility, setVisibility] = useState('private');
  const [readme, setReadme] = useState(true);
  const [branch, setBranch] = useState('');

  const create = useMutation({
    mutationFn: () =>
      api.post<Project>('/projects', {
        name,
        path: path || slug(name),
        namespace_id: namespaceId ? Number(namespaceId) : undefined,
        description: description || undefined,
        visibility,
        initialize_with_readme: readme,
        default_branch: branch || undefined,
      }),
    onSuccess: (p) => nav(localPath(p.web_url)),
  });

  const ns = namespaces.data ?? [];
  const selected = ns.find((n) => String(n.id) === namespaceId) ?? ns.find((n) => n.kind === 'user' && n.path === user?.username);

  return (
    <Layout>
      <div className="page-head">
        <h1>Create blank project</h1>
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
          <span className="label">Project name</span>
          <input
            type="text"
            value={name}
            autoFocus
            required
            placeholder="My awesome project"
            onChange={(e) => {
              setName(e.target.value);
              if (!pathEdited) setPath(slug(e.target.value));
            }}
          />
        </label>
        <div className="row" style={{ alignItems: 'flex-start' }}>
          <label className="field grow">
            <span className="label">Project URL</span>
            <select value={selected ? String(selected.id) : ''} onChange={(e) => setNamespaceId(e.target.value)}>
              {ns.map((n) => (
                <option key={n.id} value={n.id}>
                  {location.origin}/{n.full_path}/
                </option>
              ))}
            </select>
          </label>
          <span style={{ paddingTop: 32 }}>/</span>
          <label className="field grow">
            <span className="label">Project slug</span>
            <input
              type="text"
              value={path}
              required
              onChange={(e) => {
                setPath(e.target.value);
                setPathEdited(true);
              }}
            />
          </label>
        </div>
        <label className="field">
          <span className="label">Project description (optional)</span>
          <textarea rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
        </label>
        <div className="field">
          <span className="label-text">Visibility Level</span>
          {[
            ['private', 'Private', 'Project access must be granted explicitly to each user.'],
            ['internal', 'Internal', 'The project can be accessed by any signed-in user.'],
            ['public', 'Public', 'The project can be accessed without any authentication.'],
          ].map(([v, label, help]) => (
            <label key={v} className="check">
              <input type="radio" name="visibility" checked={visibility === v} onChange={() => setVisibility(v)} />
              <span>
                <strong>{label}</strong>
                <div className="help">{help}</div>
              </span>
            </label>
          ))}
        </div>
        <div className="field">
          <span className="label-text">Project Configuration</span>
          <label className="check">
            <input type="checkbox" checked={readme} onChange={(e) => setReadme(e.target.checked)} />
            <span>
              <strong>Initialize repository with a README</strong>
              <div className="help">Allows you to immediately clone this project’s repository. Skip this if you plan to push up an existing repository.</div>
            </span>
          </label>
        </div>
        <label className="field">
          <span className="label">Default branch (optional)</span>
          <input type="text" value={branch} placeholder="main" onChange={(e) => setBranch(e.target.value)} />
        </label>
        <div className="row">
          <button className="btn btn-confirm" disabled={create.isPending || !name}>
            Create project
          </button>
          <button type="button" className="btn" onClick={() => nav(-1)}>
            Cancel
          </button>
        </div>
      </form>
    </Layout>
  );
}
