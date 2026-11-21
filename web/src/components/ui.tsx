import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { Icon } from './Icon';
import { renderMarkdown } from '../lib/markdown';
import { fullDate, timeAgo } from '../lib/time';
import type { Label, MergeRequest, Pipeline, UserBasic } from '../api/types';
import { localPath } from '../lib/paths';

const avatarColors = ['#6b4fbb', '#1f75cb', '#108548', '#c17d10', '#dd2b0e', '#4b4ba3', '#0b6d75'];

export function Avatar({ name, url, size = 24, square, title }: { name: string; url?: string | null; size?: number; square?: boolean; title?: string }) {
  const [failed, setFailed] = useState(false);
  const color = avatarColors[[...name].reduce((a, c) => a + c.charCodeAt(0), 0) % avatarColors.length];
  // Gravatar is often unreachable from test setups; initials are used instead.
  const useImg = url && !failed && !url.includes('gravatar.com');
  return (
    <span
      className={`avatar${square ? ' square' : ''}`}
      style={{ width: size, height: size, fontSize: Math.max(10, size * 0.42), background: useImg ? undefined : color }}
      title={title ?? name}
    >
      {useImg ? <img src={url} alt="" onError={() => setFailed(true)} /> : name.slice(0, 1)}
    </span>
  );
}

export function UserAvatar({ user, size = 24 }: { user: UserBasic; size?: number }) {
  return <Avatar name={user.name || user.username} url={user.avatar_url} size={size} title={`${user.name} (@${user.username})`} />;
}

export function UserLink({ user, avatar = false }: { user: UserBasic; avatar?: boolean }) {
  return (
    <Link to={`/${user.username}`} className="row" style={{ display: 'inline-flex', gap: 6, color: 'inherit' }}>
      {avatar && <UserAvatar user={user} size={20} />}
      <strong style={{ color: 'var(--text)' }}>{user.name}</strong>
    </Link>
  );
}

export function Loading({ inline }: { inline?: boolean }) {
  if (inline) return <span className="spinner" />;
  return (
    <div className="loading">
      <span className="spinner" />
    </div>
  );
}

export function ErrorBox({ error }: { error: unknown }) {
  if (!error) return null;
  const msg = error instanceof Error ? error.message : String(error);
  return <div className="alert alert-danger">{msg}</div>;
}

export function TimeAgo({ at }: { at: string | null | undefined }) {
  if (!at) return null;
  return (
    <time dateTime={at} title={fullDate(at)}>
      {timeAgo(at)}
    </time>
  );
}

export function Markdown({ source, projectPath, className }: { source: string | null | undefined; projectPath?: string; className?: string }) {
  const html = useMemo(() => renderMarkdown(source ?? '', projectPath), [source, projectPath]);
  const nav = useNavigate();
  // Links within labrat stay in the app instead of reloading the page.
  const onClick = (e: React.MouseEvent) => {
    const a = (e.target as HTMLElement).closest('a');
    if (!a || e.ctrlKey || e.metaKey || e.shiftKey || e.button !== 0 || a.target) return;
    const url = new URL(a.href, location.href);
    if (url.origin !== location.origin || url.pathname.startsWith('/api/') || url.pathname.startsWith('/uploads/')) return;
    e.preventDefault();
    nav(url.pathname + url.search + url.hash);
  };
  return <div className={`md ${className ?? ''}`} onClick={onClick} dangerouslySetInnerHTML={{ __html: html }} />;
}

export function Empty({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="empty">
      <h3>{title}</h3>
      {children}
    </div>
  );
}

export function Modal({ title, onClose, children, footer }: { title: string; onClose: () => void; children: ReactNode; footer?: ReactNode }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-label={title}>
        <div className="modal-head">
          <h2>{title}</h2>
          <span className="spacer" />
          <button className="btn btn-tertiary btn-icon" onClick={onClose} aria-label="Close">
            <Icon name="close" />
          </button>
        </div>
        <div className="modal-body">{children}</div>
        {footer && <div className="modal-foot">{footer}</div>}
      </div>
    </div>
  );
}

/** A dropdown that closes on outside click. */
export function Dropdown({
  button,
  children,
  right,
  className,
}: {
  button: (open: boolean, toggle: () => void) => ReactNode;
  children: (close: () => void) => ReactNode;
  right?: boolean;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const [alignRight, setAlignRight] = useState(!!right);
  const ref = useRef<HTMLDivElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  // Open on the preferred side, but flip when the menu would leave the window.
  const flipped = useRef(false);
  useLayoutEffect(() => {
    if (!open) {
      flipped.current = false;
      setAlignRight(!!right);
      return;
    }
    const r = menu.current?.getBoundingClientRect();
    if (!r || flipped.current) return;
    if (r.left < 0 || r.right > window.innerWidth) {
      flipped.current = true;
      setAlignRight(r.left < 0 ? false : true);
    }
  }, [open, right]);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);
  return (
    <div className={`dropdown ${className ?? ''}`} ref={ref}>
      {button(open, () => setOpen((o) => !o))}
      {open && (
        <div ref={menu} className={`dropdown-menu${alignRight ? ' right' : ''}`}>
          {children(() => setOpen(false))}
        </div>
      )}
    </div>
  );
}

export function MrStateBadge({ mr }: { mr: Pick<MergeRequest, 'state' | 'draft'> }) {
  const map: Record<string, [string, string, string]> = {
    opened: ['badge-success', 'mr', 'Open'],
    merged: ['badge-info', 'mr', 'Merged'],
    closed: ['badge-danger', 'close', 'Closed'],
    locked: ['badge-neutral', 'lock', 'Locked'],
  };
  const [cls, icon, text] = map[mr.state] ?? map.opened;
  return (
    <span className={`badge ${cls}`} style={{ fontSize: 13, padding: '3px 10px' }}>
      <Icon name={icon} size={14} />
      {text}
    </span>
  );
}

const ciMap: Record<string, [string, string]> = {
  success: ['success', 'check'],
  failed: ['failed', 'close'],
  running: ['info', 'history'],
  pending: ['warning', 'history'],
  created: ['neutral', 'history'],
  canceled: ['neutral', 'close'],
  skipped: ['neutral', 'chevronRight'],
  manual: ['neutral', 'settings'],
};

export function CiStatus({ pipeline, label }: { pipeline: Pipeline | null | undefined; label?: boolean }) {
  if (!pipeline) return null;
  const [cls, icon] = ciMap[pipeline.status] ?? ['neutral', 'pipeline'];
  return (
    <a href={localPath(pipeline.web_url)} className="ci-icon" title={`Pipeline: ${pipeline.status}`} onClick={(e) => e.preventDefault()}>
      <span className={`status-icon ${cls}`} style={{ width: 16, height: 16 }}>
        <Icon name={icon} size={10} />
      </span>
      {label && <span style={{ color: 'var(--text)' }}>{pipeline.status}</span>}
    </a>
  );
}

export function LabelPill({ label, name }: { label?: Label; name?: string }) {
  const color = label?.color ?? '#dcdcde';
  const text = label?.text_color ?? '#1f1e24';
  const title = label?.name ?? name ?? '';
  const scoped = title.includes('::');
  if (scoped) {
    const i = title.lastIndexOf('::');
    return (
      <span className="gl-label" style={{ borderColor: color }} title={label?.description ?? undefined}>
        <span style={{ background: color, color: text }}>{title.slice(0, i)}</span>
        <span style={{ background: '#fff', color: 'var(--text)' }}>{title.slice(i + 2)}</span>
      </span>
    );
  }
  return (
    <span className="gl-label" style={{ background: color, color: text }} title={label?.description ?? undefined}>
      <span>{title}</span>
    </span>
  );
}

export function CopyButton({ text, title = 'Copy' }: { text: string; title?: string }) {
  const toast = useToast();
  return (
    <button
      type="button"
      className="btn btn-icon"
      title={title}
      onClick={() => {
        navigator.clipboard?.writeText(text).then(() => toast('Copied to clipboard'), () => toast('Copy failed', true));
      }}
    >
      <Icon name="copy" />
    </button>
  );
}

export function Pager({ page, totalPages, nextPage, onPage }: { page: number; totalPages: number | null; nextPage: number | null; onPage: (p: number) => void }) {
  if (page <= 1 && !nextPage) return null;
  return (
    <div className="row" style={{ justifyContent: 'center', marginTop: 16 }}>
      <button className="btn" disabled={page <= 1} onClick={() => onPage(page - 1)}>
        Previous
      </button>
      <span className="subtle">
        Page {page}
        {totalPages ? ` of ${totalPages}` : ''}
      </span>
      <button className="btn" disabled={!nextPage} onClick={() => nextPage && onPage(nextPage)}>
        Next
      </button>
    </div>
  );
}

// ---------- Toasts ----------

type ToastFn = (message: string, error?: boolean) => void;
const ToastContext = createContext<ToastFn>(() => {});

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<{ id: number; message: string; error: boolean }[]>([]);
  const show = useCallback<ToastFn>((message, error = false) => {
    const id = Date.now() + Math.random();
    setToasts((t) => [...t, { id, message, error }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), error ? 6000 : 3000);
  }, []);
  return (
    <ToastContext.Provider value={show}>
      {children}
      <div className="toast-wrap">
        {toasts.map((t) => (
          <div key={t.id} className={`toast${t.error ? ' error' : ''}`}>
            {t.message}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export const useToast = () => useContext(ToastContext);

export function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
