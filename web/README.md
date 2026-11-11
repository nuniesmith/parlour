# Parlour web UI

SvelteKit 2 / Svelte 5, built as a **static SPA** (`adapter-static`,
`ssr = false`) and embedded in the Rust server binary.

## Develop

```sh
npm install
npm run dev      # local dev server
npm run build    # outputs build/ for the server to embed
```

Nothing here is wired to the API yet — Phase 1 work.
