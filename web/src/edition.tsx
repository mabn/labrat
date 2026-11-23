import type { ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api } from './api/client';
import { useSession } from './session';

/**
 * Whether labrat runs as GitLab EE (the default) or CE (`--noee`), from `/version`'s
 * `enterprise`. `null` while unknown: `/version` needs a signed-in user.
 */
export function useEdition(): { ee: boolean | null } {
  const { user } = useSession();
  const q = useQuery({
    queryKey: ['version', user?.id ?? null],
    queryFn: () => api.get<{ enterprise: boolean }>('/version'),
    enabled: !!user,
    staleTime: Infinity,
    retry: false,
  });
  return { ee: q.data ? q.data.enterprise : null };
}

export function EeBadge() {
  return (
    <span className="badge badge-muted ee-badge" title="A GitLab EE feature">
      EE
    </span>
  );
}

/**
 * EE-only controls: shown in CE mode too, but disabled, with a note saying why. Links inside
 * still work; buttons and inputs don't.
 */
export function EeOnly({ children, note = true }: { children: ReactNode; note?: boolean }) {
  const { ee } = useEdition();
  const off = ee !== true;
  return (
    <fieldset className="ee-only" disabled={off}>
      {off && note && ee === false && <div className="ee-note small subtle">Available in EE mode only: this labrat runs as CE (<code>--noee</code>).</div>}
      <div className={off ? 'ee-off' : undefined}>{children}</div>
    </fieldset>
  );
}
