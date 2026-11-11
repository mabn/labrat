import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import { Link, NavLink, useLocation } from 'react-router-dom';
import { Icon, Logo } from './Icon';
import { Avatar, Dropdown, UserAvatar } from './ui';
import { useSession } from '../session';
import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';

export interface Crumb {
  label: string;
  to?: string;
}

export interface NavEntry {
  label: string;
  to: string;
  icon?: string;
  count?: number | string;
  sub?: NavEntry[];
  /** Match the path exactly rather than as a prefix. */
  exact?: boolean;
}

export interface SidebarContext {
  title: string;
  to: string;
  avatar?: { name: string; url?: string | null };
  nav: NavEntry[];
}

interface Chrome {
  crumbs: Crumb[];
  setCrumbs: (c: Crumb[]) => void;
}

const ChromeContext = createContext<Chrome>({ crumbs: [], setCrumbs: () => {} });

/** Holds the top bar's breadcrumbs, which pages set with `useCrumbs`. */
export function ChromeProvider({ children }: { children: ReactNode }) {
  const [crumbs, setCrumbs] = useState<Crumb[]>([]);
  return <ChromeContext.Provider value={{ crumbs, setCrumbs }}>{children}</ChromeContext.Provider>;
}

/** Sets the top bar's breadcrumbs for the current page. */
export function useCrumbs(crumbs: Crumb[]) {
  const { setCrumbs } = useContext(ChromeContext);
  const key = JSON.stringify(crumbs);
  useEffect(() => {
    setCrumbs(JSON.parse(key));
  }, [key, setCrumbs]);
}

function NavItem({ e }: { e: NavEntry }) {
  const loc = useLocation();
  const active = e.exact ? loc.pathname === e.to : loc.pathname === e.to || loc.pathname.startsWith(e.to + '/');
  const subActive = e.sub?.some((s) => loc.pathname === s.to || loc.pathname.startsWith(s.to + '/'));
  return (
    <>
      <Link to={e.to} className={`nav-item${active && !subActive ? ' active' : ''}`}>
        {e.icon && <Icon name={e.icon} />}
        <span className="truncate">{e.label}</span>
        {e.count !== undefined && e.count !== null && <span className="count">{e.count}</span>}
      </Link>
      {e.sub && (active || subActive) &&
        e.sub.map((s) => (
          <NavLink key={s.to} to={s.to} end={s.exact} className={({ isActive }) => `nav-item sub${isActive ? ' active' : ''}`}>
            <span className="truncate">{s.label}</span>
            {s.count !== undefined && <span className="count">{s.count}</span>}
          </NavLink>
        ))}
    </>
  );
}

function UserMenu() {
  const { user, signOut } = useSession();
  if (!user) {
    return (
      <Link className="btn btn-sm" to={`/users/sign_in?redirect=${encodeURIComponent(location.pathname)}`}>
        Sign in
      </Link>
    );
  }
  return (
    <Dropdown
      right
      button={(_, toggle) => (
        <button className="btn btn-tertiary btn-icon" onClick={toggle} title={user.name}>
          <UserAvatar user={user} size={24} />
        </button>
      )}
    >
      {(close) => (
        <>
          <div className="dropdown-header">
            <div style={{ color: 'var(--text)', fontSize: 14 }}>{user.name}</div>
            <div>@{user.username}</div>
          </div>
          <div className="dropdown-divider" />
          <Link className="dropdown-item" to="/-/user_settings/personal_access_tokens" onClick={close}>
            <Icon name="token" /> Access tokens
          </Link>
          <Link className="dropdown-item" to="/-/labrat/requests" onClick={close}>
            <Icon name="activity" /> Request log
          </Link>
          <div className="dropdown-divider" />
          <button className="dropdown-item" onClick={signOut}>
            <Icon name="logout" /> Sign out
          </button>
        </>
      )}
    </Dropdown>
  );
}

function CreateMenu() {
  const { user } = useSession();
  if (!user) return null;
  return (
    <Dropdown
      right
      button={(_, toggle) => (
        <button className="btn btn-tertiary btn-icon" onClick={toggle} title="Create new…">
          <Icon name="plus" />
        </button>
      )}
    >
      {(close) => (
        <>
          <Link className="dropdown-item" to="/projects/new" onClick={close}>
            New project
          </Link>
          <Link className="dropdown-item" to="/groups/new" onClick={close}>
            New group
          </Link>
          {user.is_admin && (
            <Link className="dropdown-item" to="/admin/users/new" onClick={close}>
              New user
            </Link>
          )}
        </>
      )}
    </Dropdown>
  );
}

function useCounts() {
  const { user } = useSession();
  return useQuery({
    queryKey: ['user_counts'],
    queryFn: () => api.get<{ merge_requests: number; assigned_merge_requests: number; review_requested_merge_requests: number; todos: number }>('/user_counts'),
    enabled: !!user,
    staleTime: 30_000,
  }).data;
}

export function Layout({ context, children, wide }: { context?: SidebarContext; children: ReactNode; wide?: boolean }) {
  const { crumbs } = useContext(ChromeContext);
  const { user } = useSession();
  const counts = useCounts();
  const yourWork: NavEntry[] = [
    { label: 'Projects', to: '/dashboard/projects', icon: 'project' },
    { label: 'Groups', to: '/dashboard/groups', icon: 'group' },
    { label: 'Merge requests', to: '/dashboard/merge_requests', icon: 'mr', count: counts ? counts.assigned_merge_requests + counts.review_requested_merge_requests : undefined },
    { label: 'To-Do List', to: '/dashboard/todos', icon: 'todo', count: counts?.todos },
  ];
  return (
    <>
      <div className="app">
        <aside className="sidebar">
          <div className="sidebar-top">
            <Link to="/" className="brand">
              <Logo size={24} /> labrat
            </Link>
            <span className="spacer" />
            <CreateMenu />
            <UserMenu />
          </div>
          {context ? (
            <>
              <Link to={context.to} className="sidebar-context">
                {context.avatar && <Avatar name={context.avatar.name} url={context.avatar.url} size={24} square />}
                <span className="name">{context.title}</span>
              </Link>
              <nav className="sidebar-section">
                {context.nav.map((e) => (
                  <NavItem key={e.to} e={e} />
                ))}
              </nav>
            </>
          ) : (
            <nav className="sidebar-section">
              <div className="sidebar-heading">Your work</div>
              {user ? yourWork.map((e) => <NavItem key={e.to} e={e} />) : <NavItem e={{ label: 'Explore projects', to: '/explore/projects', icon: 'project' }} />}
              {user?.is_admin && (
                <>
                  <div className="sidebar-heading">Admin</div>
                  <NavItem e={{ label: 'Users', to: '/admin/users', icon: 'users' }} />
                  <NavItem e={{ label: 'System hooks', to: '/admin/hooks', icon: 'hook' }} />
                  <NavItem e={{ label: 'Settings', to: '/admin/application_settings/network', icon: 'settings' }} />
                </>
              )}
              <div className="sidebar-heading">labrat</div>
              <NavItem e={{ label: 'Request log', to: '/-/labrat/requests', icon: 'activity' }} />
            </nav>
          )}
          <div className="sidebar-footer small subtle">GitLab-compatible test server</div>
        </aside>
        <div className="main">
          <header className="topbar">
            <nav className="breadcrumbs" aria-label="Breadcrumbs">
              {crumbs.map((c, i) => (
                <span key={i} className="row" style={{ gap: 4 }}>
                  {i > 0 && <span className="sep">/</span>}
                  {c.to ? <Link to={c.to}>{c.label}</Link> : <span style={{ color: 'var(--text)' }}>{c.label}</span>}
                </span>
              ))}
            </nav>
          </header>
          <div className={`content${wide ? ' wide' : ''}`}>{children}</div>
        </div>
      </div>
    </>
  );
}
