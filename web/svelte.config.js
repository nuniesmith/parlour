import adapter from '@sveltejs/adapter-static';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  kit: {
    // Static SPA: the Parlour server binary embeds this build output.
    adapter: adapter({ fallback: 'index.html' }),
  },
};

export default config;
