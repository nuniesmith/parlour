// Per-profile preferences (UI7): the pages under /settings edit
// the active profile through PATCH /api/v1/profiles/{id} and then
// reload the session so the sidebar, Home and player see it.

import { api } from './api';
import { loadSession, session } from './session.svelte';

export const PROFILE_COLORS = [
  '#7c5cbf',
  '#2f7fc4',
  '#3d9970',
  '#c4512f',
  '#b03a6b',
  '#8a6d2f',
  '#4d8091',
];

/** Display names for the Home Screen rail ids the server knows. */
export const RAIL_LABELS: Record<string, string> = {
  libraries: 'Libraries',
  continue_watching: 'Continue watching',
  next_up: 'Next up',
  recently_added: 'Recently added',
  latest: 'Latest in each library',
  // AB8: Audiobookshelf's book shelves (D29).
  continue_listening: 'Continue listening',
  continue_series: 'Continue series',
  recently_added_books: 'Recently added audiobooks',
  recent_series: 'Recent series',
  discover_books: 'Discover audiobooks',
  listen_again: 'Listen again',
  newest_authors: 'Newest authors',
};

/** Rails a profile can move but not hide (the server refuses it):
 *  Continue Listening is always shown (UI-AUDIO-REFERENCE §4.3). */
export const ALWAYS_SHOWN_RAILS = ['continue_listening'];

export async function updateActiveProfile(
  body: Record<string, unknown>,
): Promise<{ ok: boolean; error: string | null }> {
  const p = session.me?.active_profile;
  if (!p) return { ok: false, error: 'Pick a profile first.' };
  const { status, error } = await api(`/api/v1/profiles/${p.id}`, {
    method: 'PATCH',
    body: JSON.stringify(body),
  });
  if (status !== 200) return { ok: false, error: error ?? 'Could not save.' };
  await loadSession();
  return { ok: true, error: null };
}
