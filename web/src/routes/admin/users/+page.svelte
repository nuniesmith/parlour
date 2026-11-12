<script lang="ts">
  // Server → Users: accounts, invites, and which profiles may
  // see which libraries.
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { loadSession, session } from '$lib/session.svelte';
  import type { AdminUser, Invite, Library } from '$lib/types';

  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let users = $state<AdminUser[]>([]);
  let invites = $state<Invite[]>([]);
  let newInviteLink = $state<string | null>(null);
  let libraries = $state<Library[]>([]);
  let grantProfile = $state('');
  let grantLibrary = $state('');

  async function loadUsers() {
    const [u, i] = await Promise.all([
      api<AdminUser[]>('/api/v1/admin/users'),
      api<Invite[]>('/api/v1/admin/invites'),
    ]);
    users = u.data ?? [];
    invites = i.data ?? [];
    if (u.error) error = u.error;
  }

  onMount(async () => {
    const [, libs] = await Promise.all([
      loadUsers(),
      api<Library[]>('/api/v1/admin/libraries'),
    ]);
    libraries = libs.data ?? [];
  });

  async function createInvite() {
    error = null;
    const { data, error: err } = await api<{ invite_path: string }>('/api/v1/admin/invites', {
      method: 'POST',
      body: JSON.stringify({}),
    });
    if (data) {
      newInviteLink = `${location.origin}${data.invite_path}`;
      await loadUsers();
    } else error = err ?? 'Could not create an invite.';
  }

  async function revokeInvite(id: string) {
    await api(`/api/v1/admin/invites/${id}`, { method: 'DELETE' });
    await loadUsers();
  }

  async function grantAccess() {
    if (!grantLibrary || !grantProfile) return;
    error = null;
    notice = null;
    const { status, error: err } = await api(`/api/v1/admin/libraries/${grantLibrary}/access`, {
      method: 'POST',
      body: JSON.stringify({ profile_id: grantProfile }),
    });
    if (status === 204) {
      notice = 'Access granted.';
      // The sidebar lists the libraries the active profile can see.
      void loadSession();
    } else error = err ?? 'Could not grant access.';
  }
</script>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}<p class="notice" role="status">{notice}</p>{/if}

<h2>Users</h2>
<div class="table-wrap"><table>
  <thead><tr><th>Username</th><th>Admin</th><th>Profiles</th><th>Since</th></tr></thead>
  <tbody>
    {#each users as u (u.id)}
      <tr>
        <td>{u.username}</td>
        <td>{u.is_admin ? 'yes' : '—'}</td>
        <td>{u.profile_count}</td>
        <td class="dim">{new Date(u.created_at).toLocaleDateString()}</td>
      </tr>
    {/each}
  </tbody>
</table></div>

<h2>Invites</h2>
<p><button class="btn primary" onclick={createInvite}>Create invite</button></p>
{#if newInviteLink}
  <p class="invite-link">
    Share this link (shown once — the raw token is never stored):
    <code>{newInviteLink}</code>
  </p>
{/if}
<div class="table-wrap"><table>
  <thead><tr><th>Created</th><th>Expires</th><th>Status</th><th></th></tr></thead>
  <tbody>
    {#each invites as inv (inv.id)}
      <tr>
        <td class="dim">{new Date(inv.created_at).toLocaleDateString()}</td>
        <td class="dim">{new Date(inv.expires_at).toLocaleDateString()}</td>
        <td>{inv.used_at ? 'Used' : 'Open'}</td>
        <td>
          {#if !inv.used_at}
            <button class="btn small danger" onclick={() => revokeInvite(inv.id)}>Revoke</button>
          {/if}
        </td>
      </tr>
    {/each}
  </tbody>
</table></div>

<h2>Library access</h2>
<p class="dim">
  Access is per profile (your own profiles are listed; other users manage theirs by signing in —
  grants here apply to any profile id).
</p>
<div class="row">
  <select bind:value={grantLibrary} aria-label="Library">
    <option value="">Library…</option>
    {#each libraries as lib (lib.id)}<option value={lib.id}>{lib.name}</option>{/each}
  </select>
  <select bind:value={grantProfile} aria-label="Profile">
    <option value="">Profile…</option>
    {#each session.profiles as p (p.id)}<option value={p.id}>{p.name} (yours)</option>{/each}
  </select>
  <button class="btn" onclick={grantAccess}>Grant</button>
</div>

<style>
  .invite-link code {
    display: block;
    margin-top: 0.4rem;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.6rem;
    word-break: break-all;
  }
</style>
