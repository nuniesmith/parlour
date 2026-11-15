<script lang="ts">
  // /settings was one page until UI7 split it into a page per
  // topic. The old anchors (and the bare URL) redirect.
  import { goto } from '$app/navigation';
  import { page } from '$app/state';

  const MOVED: Record<string, string> = {
    '#playback': '/settings/playback',
    '#password': '/settings/account',
    '#sessions': '/settings/account',
  };

  $effect(() => {
    void goto(MOVED[page.url.hash] ?? '/settings/display', { replaceState: true });
  });
</script>
