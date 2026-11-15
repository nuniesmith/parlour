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
  continue_watching: 'Continue watching',
  next_up: 'Next up',
  recently_added: 'Recently added',
};

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
