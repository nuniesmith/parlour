<script lang="ts">
  // Sidebar navigation tree (UI1, D13): brand, search box, then
  // collapsible sections — Libraries, Web Settings, "<profile>
  // Preferences", and Server (admins only). The Preferences and
  // Server entries deep-link into today's /settings and /admin
  // pages; UI6/UI7 turn them into pages of their own.
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { session } from '$lib/session.svelte';

  interface NavItem {
    href: string;
    label: string;
    active: boolean;
  }
  interface NavSection {
    id: string;
    title: string;
    items: NavItem[];
    empty?: string;
  }

  let { searchInput = $bindable(null) }: { searchInput?: HTMLInputElement | null } = $props();

  const STORAGE_KEY = 'parlour.sidebar.collapsed';

  function readCollapsed(): string[] {
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      const parsed: unknown = raw ? JSON.parse(raw) : [];
      return Array.isArray(parsed) ? parsed.filter((x) => typeof x === 'string') : [];
    } catch {
      return [];
    }
  }

  let collapsed = $state<string[]>(readCollapsed());

  function toggle(id: string) {
    collapsed = collapsed.includes(id) ? collapsed.filter((x) => x !== id) : [...collapsed, id];
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(collapsed));
    } catch {
      // Private mode / blocked storage: the choice just won't persist.
    }
  }

  const path = $derived(page.url.pathname);
  const hash = $derived(page.url.hash);
  const adminTab = $derived(page.url.searchParams.get('tab') ?? 'dashboard');

  function settingsItem(anchor: string, label: string, first = false): NavItem {
    return {
      href: `/settings#${anchor}`,
      label,
      active: path === '/settings' && (hash === `#${anchor}` || (first && hash === '')),
    };
  }

  function adminItem(tab: string, label: string): NavItem {
    return {
      href: `/admin?tab=${tab}`,
      label,
      active: path === '/admin' && adminTab === tab,
    };
  }

  const sections = $derived.by((): NavSection[] => {
    const out: NavSection[] = [
      {
        id: 'libraries',
        title: 'Libraries',
        items: session.libraries.map((lib) => ({
          href: `/library/${lib.id}`,
          label: lib.name,
          active: path === `/library/${lib.id}`,
        })),
        empty: 'No libraries yet.',
      },
      {
        id: 'web',
        title: 'Web Settings',
        items: [
          { href: '/web-settings', label: 'Keyboard', active: path === '/web-settings' },
        ],
      },
    ];
    const profile = session.me?.active_profile;
    if (profile) {
      out.push({
        id: 'preferences',
        title: `${profile.name} Preferences`,
        items: [
          settingsItem('playback', 'Playback', true),
          settingsItem('password', 'Password'),
          settingsItem('sessions', 'Sessions'),
        ],
      });
    }
    if (session.me?.user.is_admin) {
      out.push({
        id: 'server',
        title: 'Server',
        items: [
          adminItem('dashboard', 'Dashboard'),
          adminItem('users', 'Users & invites'),
          adminItem('libraries', 'Libraries'),
          adminItem('jobs', 'Jobs'),
          adminItem('review', 'Metadata review'),
          adminItem('emby', 'Emby import'),
          adminItem('activity', 'Activity'),
        ],
      });
    }
    return out;
  });

  let q = $state('');

  function submitSearch(event: SubmitEvent) {
    event.preventDefault();
    const query = q.trim();
    q = '';
    void goto(query ? `/search?q=${encodeURIComponent(query)}` : '/search');
  }
</script>

<a class="brand" href="/">
  <img src="/favicon-32.png" alt="" width="28" height="28" />
  <span>Parlour</span>
</a>

<form class="search" role="search" onsubmit={submitSearch}>
  <label class="sr-only" for="sidebar-search">Search titles and people</label>
  <input
    id="sidebar-search"
    type="search"
    placeholder="Search"
    autocomplete="off"
    bind:value={q}
    bind:this={searchInput}
  />
</form>

<nav aria-label="Main">
  <a class="item" href="/" aria-current={path === '/' ? 'page' : undefined}>Home</a>

  {#each sections as section (section.id)}
    {@const open = !collapsed.includes(section.id)}
    <section>
      <button
        type="button"
        class="section-toggle"
        aria-expanded={open}
        aria-controls={`nav-${section.id}`}
        onclick={() => toggle(section.id)}
      >
        <span class="section-title">{section.title}</span>
        <span class="chevron" aria-hidden="true"></span>
      </button>
      <ul id={`nav-${section.id}`} hidden={!open}>
        {#each section.items as item (item.href)}
          <li>
            <a class="item" href={item.href} aria-current={item.active ? 'page' : undefined}
              >{item.label}</a
            >
          </li>
        {:else}
          {#if section.empty}<li class="empty">{section.empty}</li>{/if}
        {/each}
      </ul>
    </section>
  {/each}
</nav>

<style>
  .brand {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.2rem 0.6rem 0.9rem;
    font-size: 1.3rem;
    font-weight: 700;
    color: var(--accent);
    letter-spacing: 0.02em;
  }
  .brand img {
    border-radius: 6px;
  }
  .search {
    padding: 0 0.3rem 0.8rem;
  }
  .search input {
    width: 100%;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.55rem 0.8rem;
  }
  .search input::placeholder {
    color: var(--text-dim);
  }
  nav {
    display: grid;
    gap: 0.15rem;
  }
  section {
    margin-top: 0.7rem;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.15rem;
  }
  ul[hidden] {
    display: none;
  }
  .section-toggle {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    background: none;
    border: 0;
    border-radius: 0.5rem;
    padding: 0.5rem 0.75rem;
    cursor: pointer;
    color: var(--text-dim);
    font-size: 0.78rem;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    text-align: left;
  }
  .section-toggle:hover {
    color: var(--text);
  }
  .section-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chevron {
    flex: none;
    width: 0.5rem;
    height: 0.5rem;
    border-right: 2px solid currentColor;
    border-bottom: 2px solid currentColor;
    transform: rotate(45deg);
    transition: transform 120ms ease;
    margin-right: 0.2rem;
  }
  .section-toggle[aria-expanded='false'] .chevron {
    transform: rotate(-45deg);
  }
  .item {
    display: flex;
    align-items: center;
    min-height: 2.5rem;
    padding: 0.4rem 0.9rem;
    border-radius: 999px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item:hover {
    color: var(--text);
    background: var(--bg-card);
  }
  .item[aria-current='page'] {
    background: var(--accent);
    color: var(--accent-ink);
    font-weight: 600;
  }
  .empty {
    padding: 0.4rem 0.9rem;
    color: var(--text-dim);
    font-size: 0.9rem;
  }
  @media (prefers-reduced-motion: reduce) {
    .chevron {
      transition: none;
    }
  }
</style>
