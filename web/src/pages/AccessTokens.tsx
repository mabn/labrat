import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../api/client';
import type { Token } from '../api/types';
import { Layout, useCrumbs } from '../components/Layout';
import { CopyButton, Empty, ErrorBox, Loading, TimeAgo } from '../components/ui';
import { useSession } from '../session';

const SCOPES = ['api', 'read_api', 'read_user', 'read_repository', 'write_repository', 'sudo', 'admin_mode', 'create_runner', 'k8s_proxy', 'ai_features'];

export function TokenForm({ create, scopes = SCOPES, roles }: { create: (body: Record<string, unknown>) => Promise<Token>; scopes?: string[]; roles?: boolean }) {
  const [name, setName] = useState('');
  const [expires, setExpires] = useState('');
  const [chosen, setChosen] = useState<string[]>(['api']);
  const [role, setRole] = useState(30);
  const [created, setCreated] = useState<Token | null>(null);
  const qc = useQueryClient();
  const m = useMutation({
    mutationFn: () => create({ name, scopes: chosen, expires_at: expires || undefined, access_level: roles ? role : undefined }),
    onSuccess: (t) => {
      setCreated(t);
      setName('');
      qc.invalidateQueries({ queryKey: ['tokens'] });
    },
  });
  return (
    <div className="card">
      <div className="card-head">Add new token</div>
      <div className="card-body">
        {created?.token && (
          <div className="alert alert-success">
            <div style={{ marginBottom: 6 }}>Your new token has been created. Make sure you save it — you won’t be able to access it again.</div>
            <div className="clone-box">
              <input type="text" readOnly value={created.token} />
              <CopyButton text={created.token} />
            </div>
          </div>
        )}
        <ErrorBox error={m.error} />
        <form
          onSubmit={(e) => {
            e.preventDefault();
            m.mutate();
          }}
        >
          <div className="row wrap" style={{ alignItems: 'flex-start' }}>
            <label className="field grow">
              <span className="label">Token name</span>
              <input type="text" value={name} required onChange={(e) => setName(e.target.value)} />
            </label>
            <label className="field">
              <span className="label">Expiration date</span>
              <input type="date" value={expires} onChange={(e) => setExpires(e.target.value)} />
            </label>
            {roles && (
              <label className="field">
                <span className="label">Role</span>
                <select value={role} onChange={(e) => setRole(Number(e.target.value))}>
                  <option value={10}>Guest</option>
                  <option value={20}>Reporter</option>
                  <option value={30}>Developer</option>
                  <option value={40}>Maintainer</option>
                  <option value={50}>Owner</option>
                </select>
              </label>
            )}
          </div>
          <div className="field">
            <span className="label-text">Select scopes</span>
            {scopes.map((s) => (
              <label key={s} className="check">
                <input type="checkbox" checked={chosen.includes(s)} onChange={(e) => setChosen(e.target.checked ? [...chosen, s] : chosen.filter((x) => x !== s))} />
                <code>{s}</code>
              </label>
            ))}
          </div>
          <button className="btn btn-confirm" disabled={!name || !chosen.length || m.isPending}>
            Create token
          </button>
        </form>
      </div>
    </div>
  );
}

export function TokenTable({ tokens, onRevoke }: { tokens: Token[]; onRevoke: (t: Token) => void }) {
  if (!tokens.length) return <Empty title="This user has no active access tokens." />;
  return (
    <div className="table-wrap">
      <table className="table">
        <thead>
          <tr>
            <th>Token name</th>
            <th>Scopes</th>
            <th>Created</th>
            <th>Last used</th>
            <th>Expires</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {tokens.map((t) => (
            <tr key={t.id}>
              <td>
                <strong>{t.name}</strong>
              </td>
              <td className="small">{t.scopes.join(', ')}</td>
              <td className="small">
                <TimeAgo at={t.created_at} />
              </td>
              <td className="small">{t.last_used_at ? <TimeAgo at={t.last_used_at} /> : 'Never'}</td>
              <td className="small">{t.expires_at ?? 'Never'}</td>
              <td style={{ textAlign: 'right' }}>
                <button className="btn btn-sm btn-danger-secondary" onClick={() => confirm(`Revoke ${t.name}?`) && onRevoke(t)}>
                  Revoke
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function AccessTokens() {
  const { user } = useSession();
  const qc = useQueryClient();
  useCrumbs([{ label: 'User settings' }, { label: 'Personal access tokens' }]);
  const q = useQuery({ queryKey: ['tokens', 'personal'], queryFn: () => api.get<Token[]>('/personal_access_tokens', { user_id: user!.id, state: 'active', per_page: 100 }) });
  const revoke = useMutation({
    mutationFn: (t: Token) => api.del(`/personal_access_tokens/${t.id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['tokens'] }),
  });
  return (
    <Layout>
      <div className="page-head">
        <h1>Personal access tokens</h1>
      </div>
      <p className="subtle">You can generate a personal access token for each application you use that needs access to the labrat API.</p>
      {user!.is_admin ? (
        <TokenForm create={(body) => api.post<Token>(`/users/${user!.id}/personal_access_tokens`, body)} />
      ) : (
        <div className="alert alert-info">GitLab’s API lets only administrators create personal access tokens for a user. Ask an administrator, or create one with the API.</div>
      )}
      <h3 style={{ margin: '24px 0 8px' }}>Active personal access tokens</h3>
      <ErrorBox error={q.error || revoke.error} />
      {q.isLoading ? <Loading /> : <TokenTable tokens={q.data ?? []} onRevoke={(t) => revoke.mutate(t)} />}
    </Layout>
  );
}
