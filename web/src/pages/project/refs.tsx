import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Branch, Tag } from '../../api/types';
import { Dropdown } from '../../components/ui';
import { Icon } from '../../components/Icon';

export function useRefs(projectId: number) {
  return useQuery({
    queryKey: ['refs', projectId],
    queryFn: async () => {
      const [branches, tags] = await Promise.all([
        api.all<Branch>(`/projects/${projectId}/repository/branches`),
        api.all<Tag>(`/projects/${projectId}/repository/tags`),
      ]);
      return { branches, tags };
    },
    staleTime: 10_000,
  });
}

/**
 * Splits `<ref>/<path>` from a URL, where the ref may itself contain slashes: the longest
 * branch or tag name that prefixes it wins, as on GitLab.
 */
export function splitRef(parts: string[], refs: string[] | undefined, fallback: string): [string, string] {
  if (!parts.length) return [fallback, ''];
  const names = new Set(refs ?? []);
  for (let i = parts.length; i > 0; i--) {
    const candidate = parts.slice(0, i).join('/');
    if (names.has(candidate)) return [candidate, parts.slice(i).join('/')];
  }
  return [parts[0], parts.slice(1).join('/')];
}

export function decodeContent(b64: string): { text: string; binary: boolean } {
  const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  const binary = bytes.slice(0, 8000).includes(0);
  return { text: binary ? '' : new TextDecoder().decode(bytes), binary };
}

export function RefSelector({ projectId, value, onSelect }: { projectId: number; value: string; onSelect: (ref: string) => void }) {
  const refs = useRefs(projectId);
  const [search, setSearch] = useState('');
  const s = search.toLowerCase();
  return (
    <Dropdown
      button={(_, toggle) => (
        <button className="btn" onClick={toggle} style={{ maxWidth: 260 }}>
          <Icon name="branch" /> <span className="truncate">{value}</span> <Icon name="chevronDown" />
        </button>
      )}
    >
      {(close) => (
        <>
          <div className="dropdown-search">
            <input type="search" autoFocus placeholder="Search by Git revision" value={search} onChange={(e) => setSearch(e.target.value)} />
          </div>
          <div className="dropdown-header">Branches</div>
          {refs.data?.branches
            .filter((b) => b.name.toLowerCase().includes(s))
            .map((b) => (
              <button key={b.name} className={`dropdown-item${b.name === value ? ' selected' : ''}`} onClick={() => { close(); onSelect(b.name); }}>
                {b.name} {b.default && <span className="badge">default</span>}
              </button>
            ))}
          {!!refs.data?.tags.length && <div className="dropdown-header">Tags</div>}
          {refs.data?.tags
            .filter((t) => t.name.toLowerCase().includes(s))
            .map((t) => (
              <button key={t.name} className={`dropdown-item${t.name === value ? ' selected' : ''}`} onClick={() => { close(); onSelect(t.name); }}>
                <Icon name="tag" /> {t.name}
              </button>
            ))}
        </>
      )}
    </Dropdown>
  );
}

/** Navigates to the same page at another ref. */
export function useRefNav(base: string) {
  const nav = useNavigate();
  return (ref: string, path = '') => nav(`${base}/${ref}${path ? `/${path}` : ''}`);
}
