<script lang="ts">
  // <profile> Preferences (UI7): one page per topic under
  // /settings — Display, Home Screen, Playback, Subtitles, Listening (AB8),
  // Profile, Account. Everything but Account edits the active
  // profile, so those pages need one selected.
  import { page } from '$app/state';
  import { session } from '$lib/session.svelte';

  let { children } = $props();

  const needsProfile = $derived(
    page.url.pathname !== '/settings' && page.url.pathname !== '/settings/account',
  );
</script>

{#if !session.loaded}
  <p class="dim">Loading…</p>
{:else if needsProfile && !session.me?.active_profile}
  <p class="dim">
    Pick a profile first — these preferences belong to a profile.
    <a class="link" href="/profiles">Choose a profile</a>
  </p>
{:else}
  <div class="prefs">
    {@render children()}
  </div>
{/if}

<style>
  .link {
    color: var(--accent);
  }
  .prefs :global(.form) {
    max-width: 30rem;
    margin-bottom: 1.6rem;
  }
  .prefs :global(.check) {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-bottom: 0.9rem;
  }
  .prefs :global(fieldset) {
    border: 0;
    padding: 0;
    margin: 0 0 0.9rem;
  }
  .prefs :global(legend) {
    font-size: 0.85rem;
    color: var(--text-dim);
    margin-bottom: 0.3rem;
  }
  .prefs :global(.hint) {
    font-size: 0.85rem;
    color: var(--text-dim);
    margin: 0.2rem 0 0;
  }
</style>
