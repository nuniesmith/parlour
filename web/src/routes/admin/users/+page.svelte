<script lang="ts">
  // Server → Users: accounts (an admin can rename any of them —
  // UI10 — and set a new password for any of them — D37), invites,
  // which profiles may see which libraries, and (D41, POD5) which
  // profiles may see explicit podcasts.
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { loadSession, session } from '$lib/session.svelte';
  import type { AdminProfile, AdminUser, Invite, Library } from '$lib/types';

  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let users = $state<AdminUser[]>([]);
  let invites = $state<Invite[]>([]);
  let invitesLoaded = $state(false);
  let newInviteLink = $state<string | null>(null);
  let libraries = $state<Library[]>([]);
  let librariesLoaded = $state(false);
  let grantProfile = $state('');
  let grantLibrary = $state('');
  let renamingId = $state<string | null>(null);
  let renameTo = $state('');
  let renameSaving = $state(false);
  let passwordId = $state<string | null>(null);
  let newPassword = $state('');
  let passwordSaving = $state(false);
  let profiles = $state<AdminProfile[]>([]);
  let explicitSaving = $state<string | null>(null);

  async function loadProfiles() {
    const { data, error: err } = await api<AdminProfile[]>('/api/v1/admin/profiles');
    profiles = data ?? [];
    if (err) error = err;
  }

  async function setExplicit(p: AdminProfile, allow: boolean) {
    error = null;
    notice = null;
    explicitSaving = p.id;
    const { data, error: err } = await api<AdminProfile>(`/api/v1/admin/profiles/${p.id}/explicit`, {
      method: 'PUT',
      body: JSON.stringify({ allow_explicit: allow }),
    });
    explicitSaving = null;
    if (data) {
      profiles = profiles.map((x) => (x.id === data.id ? data : x));
      notice = allow
        ? `${data.username} · ${data.name} can see explicit podcasts again.`
        : `${data.username} · ${data.name} no longer sees explicit podcasts.`;
    } else {
      error = err ?? 'Could not change that profile.';
      await loadProfiles();
    }
  }

  function startPassword(u: AdminUser) {
    error = null;
    notice = null;
    renamingId = null;
    passwordId = u.id;
    newPassword = '';
  }

  async function savePassword(event: SubmitEvent, u: AdminUser) {
    event.preventDefault();
    error = null;
    notice = null;
    passwordSaving = true;
    const { status, error: err } = await api(`/api/v1/admin/users/${u.id}/password`, {
      method: 'PUT',
      body: JSON.stringify({ password: newPassword }),
    });
    passwordSaving = false;
    if (status === 204) {
      users = users.map((x) => (x.id === u.id ? { ...x, password_locked: false } : x));
      passwordId = null;
      newPassword = '';
      notice =
        u.id === session.me?.user.id
          ? 'Your password is changed. Your other devices are signed out.'
          : `${u.username} has a new password. They are signed out everywhere; tell them the new one.`;
    } else error = err ?? 'Could not set that password.';
  }

  function startRename(u: AdminUser) {
    error = null;
    notice = null;
    passwordId = null;
    renamingId = u.id;
    renameTo = u.username;
  }

  async function saveRename(event: SubmitEvent, u: AdminUser) {
    event.preventDefault();
    error = null;
    notice = null;
    const next = renameTo.trim();
    if (next === u.username) {
      renamingId = null;
      return;
    }
    renameSaving = true;
    const { data, error: err } = await api<AdminUser>(`/api/v1/admin/users/${u.id}`, {
      method: 'PATCH',
      body: JSON.stringify({ username: next }),
    });
    renameSaving = false;
    if (data) {
      users = users.map((x) => (x.id === data.id ? data : x));
      renamingId = null;
      notice = `Renamed ${u.username} to ${data.username}. Their sessions stay signed in.`;
      // Renaming yourself: the sidebar and Account page show your name.
      if (data.id === session.me?.user.id) void loadSession();
    } else error = err ?? 'Could not rename that user.';
  }

  async function loadUsers() {
    const [u, i] = await Promise.all([
      api<AdminUser[]>('/api/v1/admin/users'),
      api<Invite[]>('/api/v1/admin/invites'),
    ]);
    users = u.data ?? [];
    invites = i.data ?? [];
    if (i.data) invitesLoaded = true;
    if (u.error) error = u.error;
  }

  onMount(async () => {
    const [, libs] = await Promise.all([
      loadUsers(),
      api<Library[]>('/api/v1/admin/libraries'),
      loadProfiles(),
    ]);
    libraries = libs.data ?? [];
    if (libs.data) librariesLoaded = true;
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
  <thead><tr><th>Username</th><th>Admin</th><th>Profiles</th><th>Since</th><th></th></tr></thead>
  <tbody>
    {#each users as u (u.id)}
      <tr>
        <td>
          {#if renamingId === u.id}
            <form class="rename" onsubmit={(e) => saveRename(e, u)}>
              <input
                bind:value={renameTo}
                aria-label="New username for {u.username}"
                autocapitalize="none"
                spellcheck="false"
                required
                minlength="3"
                maxlength="32"
              />
              <button class="btn small primary" type="submit" disabled={renameSaving}>Save</button>
              <button class="btn small" type="button" onclick={() => (renamingId = null)}>
                Cancel
              </button>
            </form>
          {:else if passwordId === u.id}
            <form class="rename" onsubmit={(e) => savePassword(e, u)}>
              <span>{u.username}</span>
              <input
                type="password"
                bind:value={newPassword}
                aria-label="New password for {u.username}"
                autocomplete="new-password"
                required
                minlength="8"
                maxlength="128"
              />
              <button class="btn small primary" type="submit" disabled={passwordSaving}>Save</button>
              <button class="btn small" type="button" onclick={() => (passwordId = null)}>
                Cancel
              </button>
            </form>
          {:else}
            {u.username}
            {#if u.password_locked}<span class="badge" title="Imported without a password">No password</span>{/if}
          {/if}
        </td>
        <td>{u.is_admin ? 'yes' : '—'}</td>
        <td>{u.profile_count}</td>
        <td class="dim">{new Date(u.created_at).toLocaleDateString()}</td>
        <td>
          {#if renamingId !== u.id && passwordId !== u.id}
            <button class="btn small" onclick={() => startRename(u)}>Rename</button>
            <button class="btn small" onclick={() => startPassword(u)}>Set password</button>
          {/if}
        </td>
      </tr>
    {/each}
  </tbody>
</table></div>
<p class="dim">
  Renaming keeps the account's password, profiles and signed-in sessions. Usernames are unique
  regardless of capitals. Setting a password signs the account out everywhere else; accounts marked
  “No password” came from an import without passwords and can't sign in until you set one.
</p>

<h2>Profiles</h2>
<div class="table-wrap"><table>
  <thead><tr><th>User</th><th>Profile</th><th>Explicit podcasts</th></tr></thead>
  <tbody>
    {#each profiles as p (p.id)}
      <tr>
        <td>{p.username}</td>
        <td>{p.name}</td>
        <td>
          <label>
            <input
              type="checkbox"
              checked={p.allow_explicit}
              disabled={explicitSaving === p.id}
              onchange={(e) => setExplicit(p, (e.currentTarget as HTMLInputElement).checked)}
            />
            {p.allow_explicit ? 'Allowed' : 'Hidden'}
          </label>
        </td>
      </tr>
    {/each}
  </tbody>
</table></div>
<p class="dim">
  Explicit podcasts are allowed for every profile unless you turn them off here. Turned off, the
  profile doesn't see shows whose feed marks them explicit, or their episodes. Only an admin can
  change this.
</p>

<h2>Invites</h2>
<p><button class="btn primary" onclick={createInvite}>Create invite</button></p>
{#if newInviteLink}
  <p class="invite-link">
    Share this link (shown once — the raw token is never stored):
    <code>{newInviteLink}</code>
  </p>
{/if}
{#if invitesLoaded && invites.length === 0}
  <p class="dim">No invites yet. Create one to get a sign-up link for someone.</p>
{:else}
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
{/if}

<h2>Library access</h2>
<p class="dim">
  Access is per profile (your own profiles are listed; other users manage theirs by signing in —
  grants here apply to any profile id).
</p>
{#if librariesLoaded && libraries.length === 0}
  <p class="dim">
    There are no libraries to grant yet — <a class="link" href="/admin/library">add one under Library</a>
    first.
  </p>
{:else}
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
{/if}

<style>
  .link {
    color: var(--accent);
  }
  .rename {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .rename input {
    width: 12rem;
  }
  .badge {
    margin-left: 0.5rem;
    font-size: 0.75rem;
    padding: 0.1rem 0.45rem;
    border-radius: 0.4rem;
    border: 1px solid var(--line);
    color: var(--text-dim);
  }
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
