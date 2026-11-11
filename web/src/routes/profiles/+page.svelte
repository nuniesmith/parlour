<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';
  import { loadSession, selectProfile, session, signOut } from '$lib/session.svelte';
  import type { Profile } from '$lib/types';

  let error = $state<string | null>(null);
  let pinFor = $state<Profile | null>(null);
  let pin = $state('');
  let manage = $state(false);
  let editing = $state<Profile | 'new' | null>(null);
  let form = $state({ name: '', avatar_color: '#7c5cbf', pin: '', clear_pin: false });

  const COLORS = ['#7c5cbf', '#2f7fc4', '#3d9970', '#c4512f', '#b03a6b', '#8a6d2f', '#4d8091'];

  onMount(async () => {
    const state = await loadSession();
    if (state === 'anon') await goto('/login');
  });

  async function pick(profile: Profile) {
    error = null;
    if (profile.has_pin) {
      pinFor = profile;
      pin = '';
      return;
    }
    const r = await selectProfile(profile.id);
    if (r.ok) await goto('/');
    else error = r.error;
  }

  async function submitPin(event: SubmitEvent) {
    event.preventDefault();
    if (!pinFor) return;
    const r = await selectProfile(pinFor.id, pin);
    if (r.ok) await goto('/');
    else error = r.error ?? 'Wrong PIN.';
  }

  function startEdit(profile: Profile | 'new') {
    editing = profile;
    form = {
      name: profile === 'new' ? '' : profile.name,
      avatar_color: profile === 'new' ? '#7c5cbf' : (profile.avatar_color ?? '#7c5cbf'),
      pin: '',
      clear_pin: false,
    };
  }

  async function saveProfile(event: SubmitEvent) {
    event.preventDefault();
    error = null;
    const body: Record<string, unknown> = {
      name: form.name.trim(),
      avatar_color: form.avatar_color,
    };
    if (form.pin) body.pin = form.pin;
    if (form.clear_pin) body.clear_pin = true;
    const isNew = editing === 'new';
    const path = isNew ? '/api/v1/profiles' : `/api/v1/profiles/${(editing as Profile).id}`;
    const { status, error: err } = await api(path, {
      method: isNew ? 'POST' : 'PATCH',
      body: JSON.stringify(body),
    });
    if (status === 200 || status === 201) {
      editing = null;
      await loadSession();
    } else {
      error = err ?? 'Could not save that profile.';
    }
  }

  async function removeProfile(profile: Profile) {
    if (!confirm(`Delete profile "${profile.name}"? Its watch state goes with it.`)) return;
    const { status, error: err } = await api(`/api/v1/profiles/${profile.id}`, {
      method: 'DELETE',
    });
    if (status === 204) await loadSession();
    else error = err ?? 'Could not delete that profile.';
  }

  async function logout() {
    await signOut();
    await goto('/login');
  }
</script>

<main class="wrap">
  <h1>Who's watching?</h1>
  {#if session.me}
    <p class="dim">
      Signed in as {session.me.user.username}
      <button class="linklike" onclick={logout}>Sign out</button>
    </p>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}

  <div class="profiles">
    {#each session.profiles as profile (profile.id)}
      <div class="cell">
        <button class="profile" onclick={() => (manage ? startEdit(profile) : pick(profile))}>
          <span class="avatar" style={`background: ${profile.avatar_color ?? '#7c5cbf'}`}
            >{profile.initial}</span
          >
          <span>{profile.name}{#if profile.has_pin} 🔒{/if}</span>
        </button>
        {#if manage}
          <button class="btn small danger" onclick={() => removeProfile(profile)}>Delete</button>
        {/if}
      </div>
    {/each}
    {#if manage}
      <button class="profile" onclick={() => startEdit('new')}>
        <span class="avatar add-avatar">+</span>
        <span>New profile</span>
      </button>
    {/if}
  </div>

  {#if pinFor}
    <form class="box" onsubmit={submitPin}>
      <div class="field">
        <label for="pin">PIN for {pinFor.name}</label>
        <input id="pin" type="password" inputmode="numeric" autocomplete="off" bind:value={pin} required />
      </div>
      <div class="row">
        <button class="btn primary" type="submit">Watch</button>
        <button class="btn" type="button" onclick={() => (pinFor = null)}>Cancel</button>
      </div>
    </form>
  {/if}

  {#if editing !== null}
    <form class="box" onsubmit={saveProfile}>
      <h2>{editing === 'new' ? 'New profile' : `Edit ${editing.name}`}</h2>
      <div class="field">
        <label for="pname">Name</label>
        <input id="pname" bind:value={form.name} required maxlength="40" />
      </div>
      <div class="field">
        <span class="field-label">Colour</span>
        <div class="colors">
          {#each COLORS as c (c)}
            <button
              type="button"
              class="swatch"
              class:chosen={form.avatar_color === c}
              style={`background: ${c}`}
              aria-label={c}
              onclick={() => (form.avatar_color = c)}
            ></button>
          {/each}
        </div>
      </div>
      <div class="field">
        <label for="ppin">{editing !== 'new' && editing.has_pin ? 'New PIN (blank keeps it)' : 'PIN (optional)'}</label>
        <input id="ppin" type="password" inputmode="numeric" autocomplete="off" bind:value={form.pin} />
      </div>
      {#if editing !== 'new' && editing.has_pin}
        <label class="check">
          <input type="checkbox" bind:checked={form.clear_pin} /> Remove the PIN
        </label>
      {/if}
      <div class="row">
        <button class="btn primary" type="submit">Save</button>
        <button class="btn" type="button" onclick={() => (editing = null)}>Cancel</button>
      </div>
    </form>
  {/if}

  <p>
    <button
      class="btn"
      onclick={() => {
        manage = !manage;
        editing = null;
        pinFor = null;
      }}
    >
      {manage ? 'Done managing' : 'Manage profiles'}
    </button>
  </p>
</main>

<style>
  .wrap {
    max-width: 46rem;
    margin: 8vh auto;
    padding: 0 1rem;
    text-align: center;
  }
  .profiles {
    display: flex;
    gap: 1.4rem;
    flex-wrap: wrap;
    justify-content: center;
    margin: 2rem 0;
  }
  .cell {
    display: grid;
    gap: 0.4rem;
    justify-items: center;
  }
  .profile {
    display: grid;
    justify-items: center;
    gap: 0.5rem;
    background: none;
    border: 2px solid transparent;
    border-radius: 1rem;
    padding: 0.8rem;
    cursor: pointer;
    font-size: 1.05rem;
    color: var(--text);
  }
  .profile:hover,
  .profile:focus-visible {
    border-color: var(--accent);
  }
  .avatar {
    width: 5.2rem;
    height: 5.2rem;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: #fff;
    font-size: 2rem;
    font-weight: 600;
  }
  .add-avatar {
    background: var(--bg-raise);
    border: 2px dashed var(--line);
    color: var(--text-dim);
  }
  .linklike {
    background: none;
    border: 0;
    color: var(--accent);
    cursor: pointer;
    font-size: 1rem;
  }
  .field-label {
    font-size: 0.85rem;
    color: var(--text-dim);
  }
  .box {
    max-width: 20rem;
    margin: 1rem auto;
    text-align: left;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 1rem 1.1rem;
  }
  .row {
    display: flex;
    gap: 0.6rem;
    margin-top: 0.6rem;
  }
  .colors {
    display: flex;
    gap: 0.5rem;
  }
  .swatch {
    width: 2rem;
    height: 2rem;
    border-radius: 50%;
    border: 2px solid transparent;
    cursor: pointer;
  }
  .swatch.chosen {
    border-color: #fff;
  }
  .check {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    font-size: 0.92rem;
    margin-bottom: 0.6rem;
  }
</style>
