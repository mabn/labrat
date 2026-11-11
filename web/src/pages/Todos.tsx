import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../api/client';
import type { Todo } from '../api/types';
import { Layout, useCrumbs } from '../components/Layout';
import { Empty, ErrorBox, Loading, TimeAgo, UserAvatar } from '../components/ui';
import { localPath } from '../lib/paths';

const actions: Record<string, string> = {
  assigned: 'assigned you',
  mentioned: 'mentioned you on',
  directly_addressed: 'mentioned you on',
  review_requested: 'requested a review of',
  marked: 'added a to-do item for',
  approval_required: 'set you as an approver for',
  unmergeable: 'Could not merge',
  build_failed: 'The pipeline failed in',
  merge_train_removed: 'Removed from Merge Train:',
  member_access_requested: 'requested access to',
  review_submitted: 'reviewed',
};

export function Todos() {
  const [state, setState] = useState<'pending' | 'done'>('pending');
  const qc = useQueryClient();
  useCrumbs([{ label: 'Your work', to: '/' }, { label: 'To-Do List' }]);
  const q = useQuery({ queryKey: ['todos', state], queryFn: () => api.get<Todo[]>('/todos', { state, per_page: 100 }) });
  const done = useMutation({
    mutationFn: (id?: number) => (id ? api.post(`/todos/${id}/mark_as_done`) : api.post('/todos/mark_as_done')),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['todos'] });
      qc.invalidateQueries({ queryKey: ['user_counts'] });
    },
  });
  return (
    <Layout>
      <div className="page-head">
        <h1>To-Do List</h1>
        <div className="actions">
          {state === 'pending' && !!q.data?.length && (
            <button className="btn" onClick={() => done.mutate(undefined)}>
              Mark all as done
            </button>
          )}
        </div>
      </div>
      <div className="tabs">
        <button className={`tab${state === 'pending' ? ' active' : ''}`} onClick={() => setState('pending')}>
          To Do
        </button>
        <button className={`tab${state === 'done' ? ' active' : ''}`} onClick={() => setState('done')}>
          Done
        </button>
      </div>
      <ErrorBox error={q.error || done.error} />
      {q.isLoading ? (
        <Loading />
      ) : q.data?.length ? (
        <ul className="list list-bordered">
          {q.data.map((t) => (
            <li key={t.id} style={{ alignItems: 'center' }}>
              <UserAvatar user={t.author} size={32} />
              <div className="grow">
                <div>
                  <strong>{t.author.name}</strong> {actions[t.action_name] ?? t.action_name}{' '}
                  <Link to={localPath(t.target_url)}>
                    {t.target?.references?.full ?? ''} {t.target?.title}
                  </Link>
                </div>
                {t.body && t.body !== t.target?.title && <div className="meta truncate">{t.body}</div>}
                <div className="meta">
                  {t.project?.name_with_namespace} · <TimeAgo at={t.created_at} />
                </div>
              </div>
              {state === 'pending' && (
                <button className="btn btn-sm" onClick={() => done.mutate(t.id)}>
                  Done
                </button>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <Empty title={state === 'pending' ? 'Good job! Looks like you don’t have anything left on your To-Do List' : 'Nothing is done yet'} />
      )}
    </Layout>
  );
}
