<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';

  interface Profile {
    id: string;
    name: string;
    has_pin: boolean;
    avatar_color: string;
    initial: string;
  }
  interface Me {
    user: { username: string; is_admin: boolean };
    active_profile: Profile | null;
  }

  let me: Me | null = null;
  let profiles: Profile[] = [];
  let error: string | null = null;

  async function refresh() {
    const mine = await api<Me>('/api/v1/auth/me');
    if (mine.status === 401) {
      await goto('/login');
      return;
    }
    me = mine.data;
    const list = await api<Profile[]>('/api/v1/profiles');
    profiles = list.data ?? [];
  }

  async function select(profile: Profile) {
    let pin: string | undefined;
    if (profile.has_pin) {
      pin = prompt(`PIN for ${profile.name}`) ?? undefined;
      if (pin === undefined) return;
    }
    const { status, error: err } = await api(`/api/v1/profiles/${profile.id}/select`, {
      method: 'POST',
      body: JSON.stringify({ pin }),
    });
    if (status === 200) await refresh();
    else error = err ?? 'Could not select that profile.';
  }

  async function logout() {
    await api('/api/v1/auth/logout', { method: 'POST' });
    await goto('/login');
  }

  onMount(refresh);
</script>

<main>
  <h1>Parlour</h1>
  {#if me}
    <p>
      Signed in as <strong>{me.user.username}</strong>{#if me.user.is_admin}
        (admin){/if}
      <button class="link" onclick={logout}>Sign out</button>
    </p>
    <h2>Who's watching?</h2>
    {#if error}<p class="error">{error}</p>{/if}
    <div class="profiles">
      {#each profiles as profile (profile.id)}
        <button
          class="profile"
          class:active={me.active_profile?.id === profile.id}
          onclick={() => select(profile)}
        >
          <span class="avatar" style="background: {profile.avatar_color}"
            >{profile.initial}</span
          >
          <span>{profile.name}{#if profile.has_pin} 🔒{/if}</span>
        </button>
      {/each}
    </div>
    <p class="hint">
      The full browsing UI arrives with the catalog (Milestone 1.5). Libraries,
      sign-in and profiles are live underneath.
    </p>
  {:else}
    <p>Loading…</p>
  {/if}
</main>

<style>
  main {
    font-family: system-ui, sans-serif;
    margin: 3rem auto;
    max-width: 40rem;
    padding: 0 1rem;
  }
  .profiles {
    display: flex;
    gap: 1rem;
    flex-wrap: wrap;
  }
  .profile {
    display: grid;
    justify-items: center;
    gap: 0.4rem;
    background: none;
    border: 2px solid transparent;
    border-radius: 0.75rem;
    padding: 0.6rem;
    cursor: pointer;
    font-size: 1rem;
  }
  .profile.active {
    border-color: #7c5cbf;
  }
  .avatar {
    width: 4rem;
    height: 4rem;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: white;
    font-size: 1.6rem;
  }
  .link {
    background: none;
    border: 0;
    color: #7c5cbf;
    cursor: pointer;
    font-size: 1rem;
  }
  .hint {
    color: #888;
    font-size: 0.85rem;
  }
  .error {
    color: #b3261e;
  }
</style>
