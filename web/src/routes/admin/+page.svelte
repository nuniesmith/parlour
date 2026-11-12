<script lang="ts">
  // Server → Dashboard. Before UI6 this page held every admin
  // tab as `/admin?tab=…`; those links now redirect to the
  // page that replaced the tab.
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import ServerDashboard from '$lib/ServerDashboard.svelte';

  const MOVED: Record<string, string> = {
    dashboard: '/admin',
    users: '/admin/users',
    libraries: '/admin/library',
    jobs: '/admin/jobs',
    review: '/admin/metadata',
    emby: '/admin/library/emby-import',
    activity: '/admin/activity',
  };

  const oldTab = $derived(page.url.searchParams.get('tab'));

  $effect(() => {
    if (oldTab !== null) void goto(MOVED[oldTab] ?? '/admin', { replaceState: true });
  });
</script>

{#if oldTab === null}
  <ServerDashboard />
{/if}
