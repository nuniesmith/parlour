<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { afterNavigate, goto } from '$app/navigation';
  import { page } from '$app/state';
  import '../app.css';
  import Sidebar from '$lib/Sidebar.svelte';
  import { loadSession, session, signOut } from '$lib/session.svelte';

  let { children } = $props();

  const bare = $derived(
    page.url.pathname === '/login' ||
      page.url.pathname === '/profiles' ||
      page.url.pathname.startsWith('/invite'),
  );

  // Top-bar title. Item and player pages carry their own heading
  // in the page body, so the bar stays quiet there.
  const title = $derived.by(() => {
    const path = page.url.pathname;
    if (path === '/') return 'Home';
    if (path === '/search') return 'Search';
    if (path === '/settings') return 'Settings';
    if (path === '/web-settings') return 'Web Settings';
    if (path === '/admin') return 'Server';
    if (path.startsWith('/library/')) {
      return session.libraries.find((l) => l.id === page.params.id)?.name ?? 'Library';
    }
    return null;
  });

  // Under 1024 px the sidebar is a drawer.
  let drawerOpen = $state(false);
  let menuButton = $state<HTMLButtonElement | null>(null);
  let searchInput = $state<HTMLInputElement | null>(null);
  let canGoBack = $state(false);

  async function openDrawer() {
    drawerOpen = true;
    await tick();
    searchInput?.focus();
  }

  function closeDrawer(restoreFocus = true) {
    if (!drawerOpen) return;
    drawerOpen = false;
    if (restoreFocus) menuButton?.focus();
  }

  function onKey(event: KeyboardEvent) {
    if (event.key === 'Escape') closeDrawer();
  }

  afterNavigate((nav) => {
    closeDrawer(false);
    // Only offer Back when this tab has somewhere in-app to go back to.
    if (nav.from) canGoBack = true;
  });

  async function logout() {
    await signOut();
    await goto('/login');
  }

  onMount(async () => {
    if (bare) return;
    const state = await loadSession();
    if (state === 'anon') await goto('/login');
    else if (state === 'pick-profile') await goto('/profiles');
  });
</script>

<svelte:window onkeydown={onKey} />

{#if bare}
  {@render children()}
{:else}
  <div class="shell">
    <aside id="sidebar" class="sidebar" class:open={drawerOpen}>
      <Sidebar bind:searchInput />
    </aside>
    {#if drawerOpen}
      <button
        type="button"
        class="scrim"
        aria-label="Close menu"
        tabindex="-1"
        onclick={() => closeDrawer()}
      ></button>
    {/if}
    <div class="content">
      <header class="topbar">
        <button
          type="button"
          class="icon-btn menu"
          aria-label="Menu"
          aria-controls="sidebar"
          aria-expanded={drawerOpen}
          bind:this={menuButton}
          onclick={() => (drawerOpen ? closeDrawer() : openDrawer())}
        >
          <span class="bars" aria-hidden="true"></span>
        </button>
        {#if canGoBack && page.url.pathname !== '/'}
          <button type="button" class="icon-btn" aria-label="Back" onclick={() => history.back()}>
            <span class="arrow" aria-hidden="true"></span>
          </button>
        {/if}
        {#if title}<h1 class="title">{title}</h1>{/if}
        <div class="who">
          {#if session.me?.active_profile}
            <a class="chip-profile" href="/settings" title="Settings">
              <span
                class="avatar"
                style={`background: ${session.me.active_profile.avatar_color ?? '#7c5cbf'}`}
                >{session.me.active_profile.initial}</span
              >
              <span class="who-name">{session.me.active_profile.name}</span>
            </a>
            <a class="action" href="/profiles">Switch</a>
          {/if}
          {#if session.me}
            <button type="button" class="action" onclick={logout}>Sign out</button>
          {/if}
        </div>
      </header>
      <main>
        {#key page.url.pathname}
          {@render children()}
        {/key}
      </main>
    </div>
  </div>
{/if}

<style>
  .shell {
    --sidebar-w: 15.5rem;
    display: grid;
    grid-template-columns: var(--sidebar-w) minmax(0, 1fr);
    min-height: 100vh;
  }
  .sidebar {
    position: sticky;
    top: 0;
    height: 100vh;
    height: 100dvh;
    overflow-y: auto;
    padding: 0.9rem 0.6rem 1.5rem;
    background: var(--bg-raise);
    border-right: 1px solid var(--line);
  }
  .content {
    min-width: 0;
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    min-height: 3.5rem;
    padding: 0.5rem 1.4rem;
    border-bottom: 1px solid var(--line);
    position: sticky;
    top: 0;
    background: rgba(11, 14, 20, 0.92);
    backdrop-filter: blur(6px);
    z-index: 10;
  }
  .title {
    margin: 0;
    font-size: 1.15rem;
    font-weight: 600;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .icon-btn {
    flex: none;
    width: 2.5rem;
    height: 2.5rem;
    display: grid;
    place-items: center;
    background: none;
    border: 1px solid transparent;
    border-radius: 50%;
    cursor: pointer;
    color: var(--text-dim);
  }
  .icon-btn:hover {
    color: var(--text);
    border-color: var(--line);
  }
  .menu {
    display: none;
  }
  .bars,
  .bars::before,
  .bars::after {
    display: block;
    width: 1.15rem;
    height: 2px;
    background: currentColor;
    border-radius: 1px;
  }
  .bars {
    position: relative;
  }
  .bars::before,
  .bars::after {
    content: '';
    position: absolute;
    left: 0;
  }
  .bars::before {
    top: -6px;
  }
  .bars::after {
    top: 6px;
  }
  .arrow {
    width: 0.65rem;
    height: 0.65rem;
    border-left: 2px solid currentColor;
    border-bottom: 2px solid currentColor;
    transform: translateX(2px) rotate(45deg);
  }
  .who {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 0.9rem;
    flex: none;
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
  .action {
    background: none;
    border: 0;
    padding: 0.3rem 0.1rem;
    cursor: pointer;
    color: var(--text-dim);
    font-size: 0.9rem;
  }
  .action:hover {
    color: var(--text);
  }
  main {
    max-width: 1400px;
    margin: 0 auto;
    padding: 1.2rem 1.4rem 3rem;
  }
  .scrim {
    display: none;
  }

  @media (max-width: 1023.98px) {
    .shell {
      display: block;
    }
    .menu {
      display: grid;
    }
    .sidebar {
      position: fixed;
      inset: 0 auto 0 0;
      width: min(var(--sidebar-w), 86vw);
      z-index: 30;
      transform: translateX(-100%);
      /* Hidden from tab order and assistive tech while closed. */
      visibility: hidden;
      transition:
        transform 160ms ease,
        visibility 0s linear 160ms;
    }
    .sidebar.open {
      transform: none;
      visibility: visible;
      transition: transform 160ms ease;
      box-shadow: 0 0 2.5rem rgba(0, 0, 0, 0.6);
    }
    .scrim {
      display: block;
      position: fixed;
      inset: 0;
      z-index: 20;
      border: 0;
      padding: 0;
      background: rgba(0, 0, 0, 0.55);
      cursor: default;
    }
  }
  @media (max-width: 640px) {
    main {
      padding: 1rem 0.9rem 2.4rem;
    }
    .topbar {
      padding: 0.4rem 0.6rem;
      gap: 0.3rem;
    }
    .who {
      gap: 0.7rem;
    }
    .who-name {
      display: none;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .sidebar,
    .sidebar.open {
      transition: none;
    }
  }
</style>
