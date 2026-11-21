import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../api/client';
import { ACCESS_LEVELS, type Member, type UserBasic } from '../api/types';
import { Empty, ErrorBox, Loading, UserAvatar, useToast, errorText, TimeAgo } from './ui';
import { UserSelect } from './UserSelect';

const levels = [10, 20, 30, 40, 50];

/** Members of a project or group (`base` is `/projects/:id` or `/groups/:id`). */
export function Members({ base, canManage }: { base: string; canManage: boolean }) {
  const qc = useQueryClient();
  const toast = useToast();
  const key = ['members', base];
  const direct = useQuery({ queryKey: key, queryFn: () => api.all<Member>(`${base}/members`) });
  const inherited = useQuery({ queryKey: [...key, 'all'], queryFn: () => api.all<Member>(`${base}/members/all`) });
  const [user, setUser] = useState<UserBasic | null>(null);
  const [level, setLevel] = useState(30);
  const [expires, setExpires] = useState('');
  const refresh = () => qc.invalidateQueries({ queryKey: key });
  const add = useMutation({
    mutationFn: () => api.post(`${base}/members`, { user_id: user!.id, access_level: level, expires_at: expires || undefined }),
    onSuccess: () => {
      setUser(null);
      toast('Member added');
      refresh();
    },
  });
  const update = useMutation({
    mutationFn: ({ id, access_level }: { id: number; access_level: number }) => api.put(`${base}/members/${id}`, { access_level }),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const remove = useMutation({
    mutationFn: (id: number) => api.del(`${base}/members/${id}`),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const directIds = new Set(direct.data?.map((m) => m.id));
  const others = (inherited.data ?? []).filter((m) => !directIds.has(m.id));
  return (
    <div className="stack">
      {canManage && (
        <div className="card">
          <div className="card-head">Invite members</div>
          <div className="card-body">
            <ErrorBox error={add.error} />
            <div className="row wrap" style={{ alignItems: 'flex-end' }}>
              <div>
                <div className="label-text">User</div>
                <UserSelect value={user} onChange={setUser} />
              </div>
              <label>
                <div className="label-text">Role</div>
                <select value={level} onChange={(e) => setLevel(Number(e.target.value))} style={{ width: 160 }}>
                  {levels.map((l) => (
                    <option key={l} value={l}>
                      {ACCESS_LEVELS[l]}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <div className="label-text">Access expiration date</div>
                <input type="date" value={expires} onChange={(e) => setExpires(e.target.value)} style={{ width: 170 }} />
              </label>
              <button className="btn btn-confirm" disabled={!user || add.isPending} onClick={() => add.mutate()}>
                Invite
              </button>
            </div>
          </div>
        </div>
      )}
      <ErrorBox error={direct.error} />
      {direct.isLoading ? (
        <Loading />
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Account</th>
                <th>Source</th>
                <th>Role</th>
                <th>Expiration</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {[...(direct.data ?? []).map((m) => ({ m, inherited: false })), ...others.map((m) => ({ m, inherited: true }))].map(({ m, inherited }) => (
                <tr key={`${m.id}-${inherited}`}>
                  <td>
                    <div className="row">
                      <UserAvatar user={m} size={32} />
                      <div>
                        <strong>{m.name}</strong>
                        <div className="small subtle">@{m.username}</div>
                      </div>
                    </div>
                  </td>
                  <td className="subtle">{inherited ? 'Inherited' : 'Direct member'}</td>
                  <td>
                    {canManage && !inherited ? (
                      <select value={m.access_level} style={{ width: 150 }} onChange={(e) => update.mutate({ id: m.id, access_level: Number(e.target.value) })}>
                        {levels.map((l) => (
                          <option key={l} value={l}>
                            {ACCESS_LEVELS[l]}
                          </option>
                        ))}
                      </select>
                    ) : (
                      ACCESS_LEVELS[m.access_level] ?? m.access_level
                    )}
                  </td>
                  <td className="subtle">{m.expires_at ? <TimeAgo at={m.expires_at} /> : 'No expiration set'}</td>
                  <td style={{ textAlign: 'right' }}>
                    {canManage && !inherited && (
                      <button className="btn btn-sm btn-danger-secondary" onClick={() => confirm(`Remove ${m.name}?`) && remove.mutate(m.id)}>
                        Remove
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {!direct.data?.length && !others.length && <Empty title="No members" />}
        </div>
      )}
    </div>
  );
}
