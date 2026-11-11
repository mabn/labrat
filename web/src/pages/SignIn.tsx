import { useState } from 'react';
import { Navigate, useSearchParams } from 'react-router-dom';
import { api, setStoredToken } from '../api/client';
import type { User } from '../api/types';
import { Logo } from '../components/Icon';
import { useSession } from '../session';
import { errorText } from '../components/ui';

export function SignIn() {
  const { user } = useSession();
  const [params] = useSearchParams();
  const redirect = params.get('redirect') || '/';
  const [mode, setMode] = useState<'password' | 'token'>('password');
  const [token, setToken] = useState('');
  const [tokenError, setTokenError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (user) return <Navigate to={redirect} replace />;

  const signInWithToken = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setTokenError(null);
    setStoredToken(token.trim());
    try {
      await api.get<User>('/user');
      window.location.href = redirect;
    } catch (err) {
      setStoredToken(null);
      setTokenError(errorText(err));
      setBusy(false);
    }
  };

  return (
    <div className="auth-page">
      <div className="auth-brand">
        <Logo size={40} /> labrat
      </div>
      <div className="auth-card">
        <div className="tabs" style={{ marginTop: -8 }}>
          <button className={`tab${mode === 'password' ? ' active' : ''}`} onClick={() => setMode('password')}>
            Standard
          </button>
          <button className={`tab${mode === 'token' ? ' active' : ''}`} onClick={() => setMode('token')}>
            Access token
          </button>
        </div>
        {mode === 'password' ? (
          <form method="post" action="/users/sign_in">
            {params.get('error') && <div className="alert alert-danger">Invalid login or password.</div>}
            <input type="hidden" name="redirect" value={redirect} />
            <label className="field">
              <span className="label">Username or primary email</span>
              <input type="text" name="user[login]" autoComplete="username" autoFocus required />
            </label>
            <label className="field">
              <span className="label">Password</span>
              <input type="password" name="user[password]" autoComplete="current-password" required />
            </label>
            <button className="btn btn-confirm" style={{ width: '100%' }}>
              Sign in
            </button>
            <p className="help" style={{ marginTop: 12 }}>
              The <code>root</code> password is set with <code>--root-password</code> (default <code>labrat-root-password</code>).
            </p>
          </form>
        ) : (
          <form onSubmit={signInWithToken}>
            {tokenError && <div className="alert alert-danger">{tokenError}</div>}
            <label className="field">
              <span className="label">Personal access token</span>
              <input type="password" value={token} onChange={(e) => setToken(e.target.value)} placeholder="glpat-…" autoFocus required />
              <div className="help">Kept in this browser only. Needs the <code>api</code> scope to make changes.</div>
            </label>
            <button className="btn btn-confirm" style={{ width: '100%' }} disabled={busy}>
              Sign in with token
            </button>
          </form>
        )}
      </div>
    </div>
  );
}
