<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import '../app.css';
  import { loadSession, session } from '$lib/session.svelte';

  let { children } = $props();

  const bare = $derived(
    page.url.pathname === '/login' ||
      page.url.pathname === '/profiles' ||
      page.url.pathname.startsWith('/invite'),
  );

  onMount(async () => {
    if (bare) return;
    const state = await loadSession();
    if (state === 'anon') await goto('/login');
    else if (state === 'pick-profile') await goto('/profiles');
  });
</script>

{#if bare}
  {@render children()}
{:else}
  <header>
    <a class="brand" href="/">Parlour</a>
    <nav>
      <a href="/" aria-current={page.url.pathname === '/' ? 'page' : undefined}>Home</a>
      {#each session.libraries as lib (lib.id)}
        <a
          href={`/library/${lib.id}`}
          aria-current={page.url.pathname === `/library/${lib.id}` ? 'page' : undefined}
          >{lib.name}</a
        >
      {/each}
      <a href="/search" aria-current={page.url.pathname === '/search' ? 'page' : undefined}
        >Search</a
      >
      {#if session.me?.user.is_admin}
        <a href="/admin" aria-current={page.url.pathname === '/admin' ? 'page' : undefined}
          >Admin</a
        >
      {/if}
    </nav>
    <div class="who">
      {#if session.me?.active_profile}
        <a class="chip-profile" href="/settings">
          <span
            class="avatar"
            style={`background: ${session.me.active_profile.avatar_color ?? '#7c5cbf'}`}
            >{session.me.active_profile.initial}</span
          >
          {session.me.active_profile.name}
        </a>
        <a class="switch" href="/profiles">Switch</a>
      {/if}
    </div>
  </header>
  <main>
    {@render children()}
  </main>
{/if}

<style>
  header {
    display: flex;
    align-items: center;
    gap: 1.4rem;
    padding: 0.8rem 1.4rem;
    border-bottom: 1px solid var(--line);
    position: sticky;
    top: 0;
    background: rgba(11, 14, 20, 0.92);
    backdrop-filter: blur(6px);
    z-index: 10;
    flex-wrap: wrap;
  }
  .brand {
    font-size: 1.3rem;
    font-weight: 700;
    color: var(--accent);
    letter-spacing: 0.02em;
  }
  nav {
    display: flex;
    gap: 1.1rem;
    flex-wrap: wrap;
  }
  nav a {
    color: var(--text-dim);
    padding: 0.2rem 0.1rem;
  }
  nav a:hover {
    color: var(--text);
  }
  nav a[aria-current='page'] {
    color: var(--text);
    border-bottom: 2px solid var(--accent);
  }
  .who {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 0.9rem;
  }
  .chip-profile {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
  }
  .avatar {
    width: 1.9rem;
    height: 1.9rem;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: #fff;
    font-size: 0.95rem;
    font-weight: 600;
  }
  .switch {
    color: var(--text-dim);
    font-size: 0.9rem;
  }
  main {
    max-width: 1400px;
    margin: 0 auto;
    padding: 1.2rem 1.4rem 3rem;
  }
  @media (max-width: 640px) {
    main {
      padding: 1rem 0.9rem 2.4rem;
    }
    header {
      padding: 0.7rem 0.9rem;
      gap: 0.9rem;
    }
  }
</style>
