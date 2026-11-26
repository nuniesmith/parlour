<script lang="ts">
  // Server → Sessions: who is playing what right now (UI3 cards), and
  // every profile's listening sessions (AB8).
  import ListeningHistory from '$lib/ListeningHistory.svelte';
  import NowPlaying from '$lib/NowPlaying.svelte';
</script>

<NowPlaying />
<ListeningHistory />
