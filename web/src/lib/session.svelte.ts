// Session store: who is signed in, their profiles, the active
// profile, and the libraries that profile can see. Loaded once
// by the app shell; pages read from here instead of refetching.

import { api } from './api';
import type { Library, Me, Profile } from './types';

export const session = $state<{
  me: Me | null;
  profiles: Profile[];
  libraries: Library[];
  loaded: boolean;
}>({
  me: null,
  profiles: [],
  libraries: [],
  loaded: false,
});

export type SessionState = 'anon' | 'pick-profile' | 'ready';

export async function loadSession(): Promise<SessionState> {
  const mine = await api<Me>('/api/v1/auth/me');
  if (mine.status === 401 || !mine.data) {
    session.me = null;
    session.profiles = [];
    session.libraries = [];
    session.loaded = true;
    return 'anon';
  }
  session.me = mine.data;
  const [profiles, libraries] = await Promise.all([
    api<Profile[]>('/api/v1/profiles'),
    api<Library[]>('/api/v1/libraries'),
  ]);
  session.profiles = profiles.data ?? [];
  session.libraries = libraries.data ?? [];
  session.loaded = true;
  return session.me.active_profile ? 'ready' : 'pick-profile';
}

export async function selectProfile(
  id: string,
  pin?: string,
): Promise<{ ok: boolean; error: string | null }> {
  const { status, error } = await api(`/api/v1/profiles/${id}/select`, {
    method: 'POST',
    body: JSON.stringify({ pin }),
  });
  if (status !== 200) return { ok: false, error: error ?? 'Could not select that profile.' };
  await loadSession();
  return { ok: true, error: null };
}

export async function signOut(): Promise<void> {
  await api('/api/v1/auth/logout', { method: 'POST' });
  session.me = null;
  session.profiles = [];
  session.libraries = [];
}
