import { createContext, useContext, type ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api, ApiError, csrfToken, setStoredToken, storedToken } from './api/client';
import type { User } from './api/types';

interface Session {
  user: User | null;
  loading: boolean;
  signOut: () => void;
}

const SessionContext = createContext<Session>({ user: null, loading: true, signOut: () => {} });

export function SessionProvider({ children }: { children: ReactNode }) {
  const q = useQuery({
    queryKey: ['currentUser'],
    queryFn: async () => {
      try {
        return await api.get<User>('/user');
      } catch (e) {
        if (e instanceof ApiError && (e.status === 401 || e.status === 403)) {
          if (storedToken()) setStoredToken(null);
          return null;
        }
        throw e;
      }
    },
    staleTime: 60_000,
    retry: false,
  });

  const signOut = () => {
    if (storedToken()) {
      setStoredToken(null);
      window.location.href = '/users/sign_in';
      return;
    }
    // The web session ends with a form post, as GitLab's does.
    const form = document.createElement('form');
    form.method = 'post';
    form.action = '/users/sign_out';
    const input = document.createElement('input');
    input.type = 'hidden';
    input.name = 'authenticity_token';
    input.value = csrfToken() ?? '';
    form.appendChild(input);
    document.body.appendChild(form);
    form.submit();
  };

  return <SessionContext.Provider value={{ user: q.data ?? null, loading: q.isLoading, signOut }}>{children}</SessionContext.Provider>;
}

export const useSession = () => useContext(SessionContext);
