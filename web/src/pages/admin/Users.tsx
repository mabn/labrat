import { useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { User } from '../../api/types';
import { Layout, useCrumbs } from '../../components/Layout';
import { ErrorBox, Loading, TimeAgo, UserAvatar, useToast, errorText } from '../../components/ui';

export function AdminUsers() {
  useCrumbs([{ label: 'Admin area' }, { label: 'Users' }]);
  const qc = useQueryClient();
  const toast = useToast();
  const [search, setSearch] = useState('');
  const q = useQuery({ queryKey: ['admin-users', search], queryFn: () => api.get<User[]>('/users', { search, per_page: 100 }) });
  const action = useMutation({
    mutationFn: ({ u, a }: { u: User; a: 'block' | 'unblock' | 'deactivate' | 'activate' | 'delete' }) =>
      a === 'delete' ? api.del(`/users/${u.id}`) : api.post(`/users/${u.id}/${a}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['admin-users'] }),
    onError: (e) => toast(errorText(e), true),
  });
  return (
    <Layout>
      <div className="page-head">
        <h1>Users</h1>
        <div className="actions">
          <Link className="btn btn-confirm" to="/admin/users/new">
            New user
          </Link>
        </div>
      </div>
      <input type="search" placeholder="Search by name, email or username" value={search} onChange={(e) => setSearch(e.target.value)} style={{ marginBottom: 12 }} />
      <ErrorBox error={q.error} />
      {q.isLoading ? (
        <Loading />
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>State</th>
                <th>Created on</th>
                <th>Last activity</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {q.data?.map((u) => (
                <tr key={u.id}>
                  <td>
                    <div className="row">
                      <UserAvatar user={u} size={32} />
                      <div>
                        <Link to={`/${u.username}`}>
                          <strong>{u.name}</strong>
                        </Link>{' '}
                        {u.is_admin && <span className="badge badge-info">Admin</span>} {u.bot && <span className="badge">Bot</span>}
                        <div className="small subtle">
                          @{u.username} {u.email && `· ${u.email}`}
                        </div>
                      </div>
                    </div>
                  </td>
                  <td>
                    <span className={`badge ${u.state === 'active' ? 'badge-success' : 'badge-warning'}`}>{u.state}</span>
                  </td>
                  <td className="small">
                    <TimeAgo at={u.created_at} />
                  </td>
                  <td className="small">{(u as User & { last_activity_on?: string }).last_activity_on ?? 'Never'}</td>
                  <td style={{ textAlign: 'right', whiteSpace: 'nowrap' }}>
                    {u.username !== 'root' && (
                      <>
                        {u.state === 'blocked' ? (
                          <button className="btn btn-sm" onClick={() => action.mutate({ u, a: 'unblock' })}>
                            Unblock
                          </button>
                        ) : u.state === 'deactivated' ? (
                          <button className="btn btn-sm" onClick={() => action.mutate({ u, a: 'activate' })}>
                            Activate
                          </button>
                        ) : (
                          <>
                            <button className="btn btn-sm" onClick={() => action.mutate({ u, a: 'block' })}>
                              Block
                            </button>{' '}
                            <button className="btn btn-sm" onClick={() => action.mutate({ u, a: 'deactivate' })}>
                              Deactivate
                            </button>
                          </>
                        )}{' '}
                        <button className="btn btn-sm btn-danger-secondary" onClick={() => confirm(`Delete user ${u.username}?`) && action.mutate({ u, a: 'delete' })}>
                          Delete
                        </button>
                      </>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Layout>
  );
}

export function AdminNewUser() {
  useCrumbs([{ label: 'Admin area' }, { label: 'Users', to: '/admin/users' }, { label: 'New user' }]);
  const nav = useNavigate();
  const [f, setF] = useState({ name: '', username: '', email: '', password: '', admin: false, can_create_group: true, projects_limit: 100000 });
  const set = (k: string, v: unknown) => setF((x) => ({ ...x, [k]: v }));
  const create = useMutation({
    mutationFn: () => api.post<User>('/users', { ...f, skip_confirmation: true }),
    onSuccess: () => nav('/admin/users'),
  });
  return (
    <Layout>
      <div className="page-head">
        <h1>New user</h1>
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
          <span className="label">Name</span>
          <input type="text" required value={f.name} onChange={(e) => set('name', e.target.value)} />
        </label>
        <label className="field">
          <span className="label">Username</span>
          <input type="text" required value={f.username} onChange={(e) => set('username', e.target.value)} />
        </label>
        <label className="field">
          <span className="label">Email</span>
          <input type="email" required value={f.email} onChange={(e) => set('email', e.target.value)} />
        </label>
        <label className="field">
          <span className="label">Password</span>
          <input type="password" required minLength={8} value={f.password} onChange={(e) => set('password', e.target.value)} />
          <div className="help">At least 8 characters. The user signs in with it on this page.</div>
        </label>
        <label className="check">
          <input type="checkbox" checked={f.admin} onChange={(e) => set('admin', e.target.checked)} /> Administrator
        </label>
        <label className="check">
          <input type="checkbox" checked={f.can_create_group} onChange={(e) => set('can_create_group', e.target.checked)} /> Can create top-level group
        </label>
        <div className="row" style={{ marginTop: 16 }}>
          <button className="btn btn-confirm" disabled={create.isPending}>
            Create user
          </button>
          <Link className="btn" to="/admin/users">
            Cancel
          </Link>
        </div>
      </form>
    </Layout>
  );
}
