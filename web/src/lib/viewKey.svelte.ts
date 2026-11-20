// The root layout mounts each page afresh when the URL path changes
// (`{#key}` in `routes/+layout.svelte`): pages load what they show when
// they mount. The one exception is the player rolling into the next
// episode (SEG6, D23). It swaps the episode in place — so playback and
// full screen carry on — and moves the URL along with a real navigation
// that replaces the history entry, so Back, Forward and reload all land
// on the episode the URL names. (SvelteKit's shallow `replaceState`
// would keep the old URL as the page's own, and Forward would then
// reopen the old episode under the new address.) The player tells the
// layout which paths it has carried its page to; those keep the key the
// page was mounted under until that page goes away.

const carried = $state<{ key: string | null; paths: string[] }>({ key: null, paths: [] });

/** The key the root layout mounts the page for `pathname` under. */
export function viewKey(pathname: string): string {
  return carried.key !== null && carried.paths.includes(pathname) ? carried.key : pathname;
}

/** The page mounted under `key` is moving its URL to `pathname` and
 *  stays mounted. Call this before navigating there. */
export function carryView(key: string, pathname: string): void {
  if (carried.key !== key) {
    carried.key = key;
    carried.paths = [];
  }
  if (!carried.paths.includes(pathname)) carried.paths.push(pathname);
}

/** The page mounted under `key` is gone: every path mounts its own
 *  page again. */
export function releaseView(key: string): void {
  if (carried.key === key) {
    carried.key = null;
    carried.paths = [];
  }
}
