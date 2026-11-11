import { useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Layout, useCrumbs } from '../components/Layout';
import { Empty, ErrorBox, Loading, TimeAgo } from '../components/ui';

interface Entry {
  id: number;
  at: string;
  method: string;
  uri: string;
  user: string | null;
  status: number;
  duration_ms: number;
  request_headers: [string, string][];
  request_body: string | null;
  response_body: string | null;
}

const pretty = (s: string | null) => {
  if (!s) return '';
  try {
    return JSON.stringify(JSON.parse(s), null, 2);
  } catch {
    return s;
  }
};

const statusClass = (s: number) => (s >= 500 ? 'badge-danger' : s >= 400 ? 'badge-warning' : 'badge-success');

/** labrat's log of API calls made by clients under test (the UI's own calls aren't logged). */
export function RequestLog() {
  useCrumbs([{ label: 'labrat' }, { label: 'Request log' }]);
  const qc = useQueryClient();
  const [open, setOpen] = useState<number | null>(null);
  const [filter, setFilter] = useState('');
  const [auto, setAuto] = useState(true);
  const q = useQuery({
    queryKey: ['request-log'],
    queryFn: async () => {
      const r = await fetch('/-/labrat/requests.json?limit=500');
      if (!r.ok) throw new Error(`Request failed (${r.status})`);
      return (await r.json()) as Entry[];
    },
    refetchInterval: auto ? 3000 : false,
  });
  const clear = async () => {
    await fetch('/-/labrat/requests.json', { method: 'DELETE' });
    qc.invalidateQueries({ queryKey: ['request-log'] });
  };
  const list = (q.data ?? []).filter((e) => !filter || `${e.method} ${e.uri} ${e.status} ${e.user ?? ''}`.toLowerCase().includes(filter.toLowerCase()));
  return (
    <Layout>
      <div className="page-head">
        <h1>Request log</h1>
        <div className="actions">
          <label className="row small">
            <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} /> Auto-refresh
          </label>
          <button className="btn" onClick={clear}>
            Clear
          </button>
        </div>
      </div>
      <p className="subtle">The last 500 calls to the REST and GraphQL APIs, newest first. Calls made by this web UI aren’t recorded.</p>
      <input type="search" placeholder="Filter by method, path, status or user" value={filter} onChange={(e) => setFilter(e.target.value)} style={{ marginBottom: 12 }} />
      <ErrorBox error={q.error} />
      {q.isLoading ? (
        <Loading />
      ) : list.length ? (
        <ul className="list list-bordered">
          {list.map((e) => (
            <li key={e.id} style={{ display: 'block', cursor: 'pointer' }} onClick={() => setOpen(open === e.id ? null : e.id)}>
              <div className="row">
                <span className="req-method">{e.method}</span>
                <span className={`badge ${statusClass(e.status)}`}>{e.status}</span>
                <code className="grow truncate">{e.uri}</code>
                <span className="small subtle">{e.user ?? 'anonymous'}</span>
                <span className="small subtle" style={{ width: 60, textAlign: 'right' }}>
                  {e.duration_ms < 10 ? e.duration_ms.toFixed(1) : Math.round(e.duration_ms)} ms
                </span>
                <span className="small subtle" style={{ width: 110, textAlign: 'right' }}>
                  <TimeAgo at={e.at} />
                </span>
              </div>
              {open === e.id && (
                <div className="req-detail" onClick={(ev) => ev.stopPropagation()} style={{ marginTop: 12, cursor: 'auto' }}>
                  <div className="label-text">Request headers</div>
                  <pre>{e.request_headers.map(([k, v]) => `${k}: ${v}`).join('\n')}</pre>
                  {e.request_body && (
                    <>
                      <div className="label-text">Request body</div>
                      <pre>{pretty(e.request_body)}</pre>
                    </>
                  )}
                  <div className="label-text">Response body</div>
                  <pre>{pretty(e.response_body)}</pre>
                </div>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <Empty title="No API calls recorded yet" />
      )}
    </Layout>
  );
}
