import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../api/client';
import type { Hook, HookEvent } from '../api/types';
import { Dropdown, Empty, ErrorBox, Loading, Modal, useToast, errorText, TimeAgo } from './ui';
import { Icon } from './Icon';

const PROJECT_EVENTS: [keyof Hook, string, string][] = [
  ['push_events', 'Push events', 'A push is made to the repository.'],
  ['tag_push_events', 'Tag push events', 'A new tag is pushed to the repository.'],
  ['note_events', 'Comments', 'A comment is made or edited on a merge request or commit.'],
  ['merge_requests_events', 'Merge request events', 'A merge request is created, updated, or merged.'],
  ['pipeline_events', 'Pipeline events', 'A pipeline status changes.'],
  ['emoji_events', 'Emoji events', 'An emoji is awarded or revoked.'],
];

const SYSTEM_EVENTS: [string, string, string][] = [
  ['push_events', 'Push events', 'A push is made to a repository.'],
  ['tag_push_events', 'Tag push events', 'A tag is pushed.'],
  ['merge_requests_events', 'Merge request events', 'A merge request is created, updated, or merged.'],
  ['repository_update_events', 'Repository update events', 'A repository is updated.'],
];

const TRIGGERS = ['push_events', 'tag_push_events', 'note_events', 'merge_requests_events', 'emoji_events'];

function HookForm({ hook, system, onSave, onClose, busy, error }: { hook?: Hook; system: boolean; onSave: (body: Record<string, unknown>) => void; onClose: () => void; busy: boolean; error: unknown }) {
  const events = system ? SYSTEM_EVENTS : PROJECT_EVENTS;
  const [url, setUrl] = useState(hook?.url ?? '');
  const [name, setName] = useState(hook?.name ?? '');
  const [token, setToken] = useState('');
  const [ssl, setSsl] = useState(hook?.enable_ssl_verification ?? true);
  const [filter, setFilter] = useState(hook?.push_events_branch_filter ?? '');
  const [flags, setFlags] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(events.map(([k]) => [k, hook ? Boolean((hook as unknown as Record<string, unknown>)[k as string]) : k === 'push_events'])),
  );
  return (
    <Modal
      title={hook ? 'Edit webhook' : 'Add new webhook'}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn btn-confirm"
            disabled={!url || busy}
            onClick={() =>
              onSave({
                url,
                ...(system ? {} : { name: name || undefined }),
                token: token || undefined,
                enable_ssl_verification: ssl,
                push_events_branch_filter: filter || undefined,
                ...flags,
              })
            }
          >
            {hook ? 'Save changes' : 'Add webhook'}
          </button>
        </>
      }
    >
      <ErrorBox error={error} />
      {!system && (
        <label className="field">
          <span className="label">Name (optional)</span>
          <input type="text" value={name} onChange={(e) => setName(e.target.value)} />
        </label>
      )}
      <label className="field">
        <span className="label">URL</span>
        <input type="url" value={url} autoFocus placeholder="http://example.com/trigger-ci.json" onChange={(e) => setUrl(e.target.value)} />
        <div className="help">As on GitLab, URLs on the local network are refused unless an administrator allows them (Admin area → Settings).</div>
      </label>
      <label className="field">
        <span className="label">Secret token</span>
        <input type="text" value={token} placeholder={hook ? 'Unchanged' : ''} onChange={(e) => setToken(e.target.value)} />
        <div className="help">Sent in the X-Gitlab-Token header.</div>
      </label>
      <div className="field">
        <span className="label-text">Trigger</span>
        {events.map(([k, label, help]) => (
          <label key={k as string} className="check">
            <input type="checkbox" checked={!!flags[k as string]} onChange={(e) => setFlags({ ...flags, [k as string]: e.target.checked })} />
            <span>
              <strong>{label}</strong>
              <div className="help">{help}</div>
            </span>
          </label>
        ))}
      </div>
      <label className="field">
        <span className="label">Push events branch filter</span>
        <input type="text" value={filter} placeholder="All branches" onChange={(e) => setFilter(e.target.value)} />
      </label>
      <label className="check">
        <input type="checkbox" checked={ssl} onChange={(e) => setSsl(e.target.checked)} /> Enable SSL verification
      </label>
    </Modal>
  );
}

function HookEvents({ base, hook, onClose }: { base: string; hook: Hook; onClose: () => void }) {
  const toast = useToast();
  const qc = useQueryClient();
  const key = ['hook-events', base, hook.id];
  const q = useQuery({ queryKey: key, queryFn: () => api.get<HookEvent[]>(`${base}/hooks/${hook.id}/events`, { per_page: 50 }) });
  const [open, setOpen] = useState<number | null>(null);
  const resend = useMutation({
    mutationFn: (id: number) => api.post(`${base}/hooks/${hook.id}/events/${id}/resend`),
    onSuccess: () => {
      toast('Request resent');
      qc.invalidateQueries({ queryKey: key });
    },
    onError: (e) => toast(errorText(e), true),
  });
  return (
    <Modal title="Recent events" onClose={onClose}>
      {q.isLoading ? (
        <Loading />
      ) : q.data?.length ? (
        <ul className="list list-bordered">
          {q.data.map((e) => (
            <li key={e.id} style={{ display: 'block' }}>
              <div className="row">
                <span className={`badge ${String(e.response_status).startsWith('2') ? 'badge-success' : 'badge-danger'}`}>{e.response_status}</span>
                <span className="grow">{e.trigger.replace(/_/g, ' ')}</span>
                <span className="small subtle">{e.execution_duration.toFixed(2)} sec</span>
                {e.created_at && (
                  <span className="small subtle">
                    <TimeAgo at={e.created_at} />
                  </span>
                )}
                <button className="btn btn-sm" onClick={() => setOpen(open === e.id ? null : e.id)}>
                  View details
                </button>
                <button className="btn btn-sm" onClick={() => resend.mutate(e.id)}>
                  Resend
                </button>
              </div>
              {open === e.id && (
                <div className="req-detail" style={{ marginTop: 8 }}>
                  <div className="label-text">Request headers</div>
                  <pre>{Object.entries(e.request_headers).map(([k, v]) => `${k}: ${v}`).join('\n')}</pre>
                  <div className="label-text">Request body</div>
                  <pre>{JSON.stringify(e.request_data, null, 2)}</pre>
                  <div className="label-text">Response body</div>
                  <pre>{e.response_body}</pre>
                </div>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <Empty title="No webhook events" />
      )}
    </Modal>
  );
}

/** Project webhooks (`base` = `/projects/:id`) or system hooks (`base` = ''). */
export function Webhooks({ base }: { base: string }) {
  const system = base === '';
  const qc = useQueryClient();
  const toast = useToast();
  const key = ['hooks', base];
  const q = useQuery({ queryKey: key, queryFn: () => api.get<Hook[]>(`${base}/hooks`, { per_page: 100 }) });
  const [editing, setEditing] = useState<Hook | 'new' | null>(null);
  const [events, setEvents] = useState<Hook | null>(null);
  const save = useMutation({
    mutationFn: (body: Record<string, unknown>) => (editing === 'new' ? api.post(`${base}/hooks`, body) : api.put(`${base}/hooks/${(editing as Hook).id}`, body)),
    onSuccess: () => {
      setEditing(null);
      qc.invalidateQueries({ queryKey: key });
    },
  });
  const remove = useMutation({
    mutationFn: (h: Hook) => api.del(`${base}/hooks/${h.id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: key }),
    onError: (e) => toast(errorText(e), true),
  });
  const test = useMutation({
    mutationFn: ({ h, trigger }: { h: Hook; trigger: string }) => (system ? api.post(`/hooks/${h.id}`) : api.post(`${base}/hooks/${h.id}/test/${trigger}`)),
    onSuccess: () => toast('Test request sent'),
    onError: (e) => toast(errorText(e), true),
  });
  return (
    <>
      <div className="page-head">
        <h1>{system ? 'System hooks' : 'Webhooks'}</h1>
        <div className="actions">
          <button className="btn btn-confirm" onClick={() => setEditing('new')}>
            Add new webhook
          </button>
        </div>
      </div>
      <p className="subtle">Webhooks send HTTP POST requests to a URL when events happen. labrat sends GitLab’s payloads and headers.</p>
      <ErrorBox error={q.error} />
      {q.isLoading ? (
        <Loading />
      ) : q.data?.length ? (
        <ul className="list list-bordered">
          {q.data.map((h) => (
            <li key={h.id} style={{ alignItems: 'center' }}>
              <div className="grow">
                <div className="title truncate">{h.name || h.url}</div>
                {h.name && <div className="meta truncate">{h.url}</div>}
                <div className="row wrap" style={{ marginTop: 4, gap: 4 }}>
                  {(system ? SYSTEM_EVENTS : PROJECT_EVENTS)
                    .filter(([k]) => (h as unknown as Record<string, unknown>)[k as string])
                    .map(([k, label]) => (
                      <span key={k as string} className="badge">
                        {label}
                      </span>
                    ))}
                  <span className="badge badge-muted">SSL Verification: {h.enable_ssl_verification ? 'enabled' : 'disabled'}</span>
                </div>
              </div>
              <Dropdown
                right
                button={(_, toggle) => (
                  <button className="btn btn-sm" onClick={toggle}>
                    Test <Icon name="chevronDown" size={12} />
                  </button>
                )}
              >
                {(close) =>
                  (system ? ['push_events'] : TRIGGERS).map((t) => (
                    <button
                      key={t}
                      className="dropdown-item"
                      onClick={() => {
                        close();
                        test.mutate({ h, trigger: t });
                      }}
                    >
                      {t.replace(/_/g, ' ')}
                    </button>
                  ))
                }
              </Dropdown>
              {!system && (
                <button className="btn btn-sm" onClick={() => setEvents(h)}>
                  Events
                </button>
              )}
              <button className="btn btn-sm" onClick={() => setEditing(h)}>
                Edit
              </button>
              <button className="btn btn-sm btn-danger-secondary" onClick={() => confirm('Delete this webhook?') && remove.mutate(h)}>
                Delete
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <Empty title="No webhooks enabled. Select trigger events above." />
      )}
      {editing && <HookForm hook={editing === 'new' ? undefined : editing} system={system} onClose={() => setEditing(null)} onSave={(b) => save.mutate(b)} busy={save.isPending} error={save.error} />}
      {events && <HookEvents base={base} hook={events} onClose={() => setEvents(null)} />}
    </>
  );
}
