<script lang="ts">
  // Preferences → Account: things that belong to the login, not
  // the profile — username (UI10), password, signed-in devices,
  // switch / sign out.
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';
  import { formatDateTime } from '$lib/types';
  import { loadSession, session, signOut } from '$lib/session.svelte';

  interface SessionRow {
    id: string;
    created_at: string;
    expires_at: string;
  }

  let sessions = $state<SessionRow[]>([]);
  let notice = $state<string | null>(null);
  let pwForm = $state({ current: '', next: '', confirm: '' });
  let pwError = $state<string | null>(null);
  let pwNotice = $state<string | null>(null);
  let pwSaving = $state(false);
  let unForm = $state({ next: '', password: '' });
  let unError = $state<string | null>(null);
  let unNotice = $state<string | null>(null);
  let unSaving = $state(false);

  async function loadSessions() {
    const { data } = await api<SessionRow[]>('/api/v1/auth/sessions');
    sessions = data ?? [];
  }

  onMount(loadSessions);

  async function changePassword(event: SubmitEvent) {
    event.preventDefault();
    pwError = null;
    pwNotice = null;
    if (pwForm.next.length < 8) {
      pwError = 'New password must be at least 8 characters.';
      return;
    }
    if (pwForm.next !== pwForm.confirm) {
      pwError = 'New password and confirmation do not match.';
      return;
    }
    if (pwForm.next === pwForm.current) {
      pwError = 'New password must be different from your current password.';
      return;
    }
    pwSaving = true;
    const { status, error: err } = await api('/api/v1/auth/change-password', {
      method: 'POST',
      body: JSON.stringify({
        current_password: pwForm.current,
        new_password: pwForm.next,
      }),
    });
    pwSaving = false;
    if (status === 204) {
      pwForm = { current: '', next: '', confirm: '' };
      pwNotice = 'Password changed. Your other sessions have been signed out.';
      await loadSessions();
    } else {
      pwError = err ?? 'Could not change password.';
    }
  }

  // Same rules as the server: 3-32 of letters, digits, _ . -
  const USERNAME_RE = /^[A-Za-z0-9_.-]{3,32}$/;

  async function changeUsername(event: SubmitEvent) {
    event.preventDefault();
    unError = null;
    unNotice = null;
    const next = unForm.next.trim();
    if (!USERNAME_RE.test(next)) {
      unError = 'Username must be 3-32 characters: letters, digits, _ . -';
      return;
    }
    if (next === session.me?.user.username) {
      unError = 'That is already your username.';
      return;
    }
    unSaving = true;
    const { status, error: err } = await api('/api/v1/auth/change-username', {
      method: 'POST',
      body: JSON.stringify({ current_password: unForm.password, new_username: next }),
    });
    unSaving = false;
    if (status === 200) {
      unForm = { next: '', password: '' };
      unNotice = `Username changed to ${next}. Sign in with it from now on — you stay signed in everywhere.`;
      await loadSession();
    } else {
      unError = err ?? 'Could not change username.';
    }
  }

  async function revoke(id: string) {
    const { status } = await api(`/api/v1/auth/sessions/${id}`, { method: 'DELETE' });
    if (status === 204) {
      sessions = sessions.filter((s) => s.id !== id);
      notice = 'Session revoked.';
    }
  }

  async function logout() {
    await signOut();
    await goto('/login');
  }
</script>

<h2>Account</h2>
<p class="dim">Signed in as {session.me?.user.username}.</p>
<p>
  <a class="btn" href="/profiles">Switch profile</a>
  <button class="btn" onclick={logout}>Sign out</button>
</p>

<h3 id="username">Change username</h3>
<form onsubmit={changeUsername} class="form">
  {#if unError}<p class="error" role="alert">{unError}</p>{/if}
  {#if unNotice}<p class="notice" role="status">{unNotice}</p>{/if}
  <div class="field">
    <label for="un-new">New username (3-32: letters, digits, _ . -)</label>
    <input
      id="un-new"
      type="text"
      bind:value={unForm.next}
      autocomplete="username"
      autocapitalize="none"
      spellcheck="false"
      required
      minlength="3"
      maxlength="32"
    />
  </div>
  <div class="field">
    <label for="un-password">Current password</label>
    <input
      id="un-password"
      type="password"
      bind:value={unForm.password}
      autocomplete="current-password"
      required
    />
  </div>
  <button class="btn primary" type="submit" disabled={unSaving}>Change username</button>
  <p class="dim">
    Usernames are unique regardless of capitals. Your password, profiles and signed-in devices
    stay as they are.
  </p>
</form>

<h3 id="password">Change password</h3>
<form onsubmit={changePassword} class="form">
  {#if pwError}<p class="error" role="alert">{pwError}</p>{/if}
  {#if pwNotice}<p class="notice" role="status">{pwNotice}</p>{/if}
  <div class="field">
    <label for="pw-current">Current password</label>
    <input
      id="pw-current"
      type="password"
      bind:value={pwForm.current}
      autocomplete="current-password"
      required
    />
  </div>
  <div class="field">
    <label for="pw-new">New password (8 or more characters)</label>
    <input
      id="pw-new"
      type="password"
      bind:value={pwForm.next}
      autocomplete="new-password"
      required
      minlength="8"
    />
  </div>
  <div class="field">
    <label for="pw-confirm">Confirm new password</label>
    <input
      id="pw-confirm"
      type="password"
      bind:value={pwForm.confirm}
      autocomplete="new-password"
      required
    />
  </div>
  <button class="btn primary" type="submit" disabled={pwSaving}>Change password</button>
  <p class="dim">Changing your password signs out your other sessions.</p>
</form>

<h3 id="sessions">Sessions</h3>
<p class="dim">Devices signed in as {session.me?.user.username}.</p>
{#if notice}<p class="notice" role="status">{notice}</p>{/if}
{#if sessions.length === 0}
  <p class="dim">No other sessions.</p>
{:else}
  <ul class="sessions">
    {#each sessions as s (s.id)}
      <li>
        <span class="dim">since {formatDateTime(s.created_at)}</span>
        <button class="btn small danger" onclick={() => revoke(s.id)}>Revoke</button>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .sessions {
    list-style: none;
    padding: 0;
    display: grid;
    gap: 0.5rem;
    max-width: 34rem;
  }
  .sessions li {
    display: flex;
    justify-content: space-between;
    align-items: center;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.5rem 0.8rem;
  }
</style>
