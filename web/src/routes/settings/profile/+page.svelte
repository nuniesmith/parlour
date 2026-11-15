<script lang="ts">
  // Preferences → Profile: the active profile's name, avatar
  // colour and PIN. Adding or deleting profiles stays on the
  // Who's watching screen.
  import { session } from '$lib/session.svelte';
  import { PROFILE_COLORS, updateActiveProfile } from '$lib/preferences';

  const p = session.me?.active_profile;
  let form = $state({
    name: p?.name ?? '',
    avatar_color: p?.avatar_color ?? PROFILE_COLORS[0],
    pin: '',
    clear_pin: false,
  });
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);
  let saving = $state(false);
  const hasPin = $derived(session.me?.active_profile?.has_pin ?? false);

  async function save(event: SubmitEvent) {
    event.preventDefault();
    error = null;
    notice = null;
    const body: Record<string, unknown> = {
      name: form.name.trim(),
      avatar_color: form.avatar_color,
    };
    if (form.clear_pin) body.clear_pin = true;
    else if (form.pin) body.pin = form.pin;
    saving = true;
    const r = await updateActiveProfile(body);
    saving = false;
    if (r.ok) {
      notice = 'Saved.';
      form.pin = '';
      form.clear_pin = false;
    } else {
      error = r.error;
    }
  }
</script>

<h2>Profile</h2>

<form class="form" onsubmit={save}>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  <div class="field">
    <label for="pname">Name</label>
    <input id="pname" bind:value={form.name} required maxlength="40" />
  </div>
  <fieldset>
    <legend>Colour</legend>
    <div class="colors">
      {#each PROFILE_COLORS as c (c)}
        <button
          type="button"
          class="swatch"
          class:chosen={form.avatar_color === c}
          style={`background: ${c}`}
          aria-label={c}
          aria-pressed={form.avatar_color === c}
          onclick={() => (form.avatar_color = c)}
        ></button>
      {/each}
    </div>
  </fieldset>
  <div class="field">
    <label for="ppin">{hasPin ? 'New PIN (blank keeps the current one)' : 'PIN (optional)'}</label>
    <input
      id="ppin"
      type="password"
      inputmode="numeric"
      autocomplete="off"
      bind:value={form.pin}
      disabled={form.clear_pin}
    />
  </div>
  {#if hasPin}
    <label class="check">
      <input type="checkbox" bind:checked={form.clear_pin} /> Remove the PIN
    </label>
  {/if}
  <p><button class="btn primary" type="submit" disabled={saving}>Save</button></p>
</form>

<p class="dim">
  To add or delete profiles, use <a class="link" href="/profiles">Who's watching → Manage profiles</a>.
</p>

<style>
  .link {
    color: var(--accent);
  }
  .colors {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
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
</style>
