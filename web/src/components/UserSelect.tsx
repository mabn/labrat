import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import type { UserBasic } from '../api/types';
import { Dropdown, UserAvatar } from './ui';
import { Icon } from './Icon';

/** Picks one user from a search over `/users` (or a given list endpoint). */
export function UserSelect({
  value,
  onChange,
  endpoint = '/users',
  placeholder = 'Select a user',
}: {
  value: UserBasic | null;
  onChange: (u: UserBasic | null) => void;
  endpoint?: string;
  placeholder?: string;
}) {
  const [search, setSearch] = useState('');
  const q = useQuery({
    queryKey: ['user-search', endpoint, search],
    queryFn: () => api.get<UserBasic[]>(endpoint, { search, per_page: 20, active: endpoint === '/users' ? true : undefined }),
  });
  return (
    <Dropdown
      button={(_, toggle) => (
        <button type="button" className="btn" style={{ minWidth: 240, justifyContent: 'space-between' }} onClick={toggle}>
          {value ? (
            <span className="row">
              <UserAvatar user={value} size={18} /> {value.name} <span className="subtle">@{value.username}</span>
            </span>
          ) : (
            <span className="subtle">{placeholder}</span>
          )}
          <Icon name="chevronDown" />
        </button>
      )}
    >
      {(close) => (
        <>
          <div className="dropdown-search">
            <input type="search" autoFocus placeholder="Search users" value={search} onChange={(e) => setSearch(e.target.value)} />
          </div>
          {q.data?.map((u) => (
            <button
              type="button"
              key={u.id}
              className="dropdown-item"
              onClick={() => {
                onChange(u);
                close();
              }}
            >
              <UserAvatar user={u} size={20} /> {u.name} <span className="subtle">@{u.username}</span>
            </button>
          ))}
          {q.data?.length === 0 && <div className="dropdown-header">No matching users</div>}
        </>
      )}
    </Dropdown>
  );
}

/** Toggles several users (assignees, reviewers) from a list of candidates. */
export function MultiUserSelect({
  candidates,
  selected,
  onToggle,
  close,
}: {
  candidates: UserBasic[];
  selected: number[];
  onToggle: (u: UserBasic) => void;
  close?: () => void;
}) {
  const [search, setSearch] = useState('');
  const s = search.toLowerCase();
  const list = candidates.filter((u) => !s || u.name.toLowerCase().includes(s) || u.username.toLowerCase().includes(s));
  return (
    <>
      <div className="dropdown-search">
        <input type="search" autoFocus placeholder="Search users" value={search} onChange={(e) => setSearch(e.target.value)} onKeyDown={(e) => e.key === 'Escape' && close?.()} />
      </div>
      {list.map((u) => (
        <button type="button" key={u.id} className={`dropdown-item${selected.includes(u.id) ? ' selected' : ''}`} onClick={() => onToggle(u)}>
          <span style={{ width: 16 }}>{selected.includes(u.id) && <Icon name="check" />}</span>
          <UserAvatar user={u} size={20} /> {u.name} <span className="subtle">@{u.username}</span>
        </button>
      ))}
      {list.length === 0 && <div className="dropdown-header">No matching users</div>}
    </>
  );
}
