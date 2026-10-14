import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// Deployed to https://0funct0ry.github.io/microterm/ (SPEC §16).
export default defineConfig({
  site: 'https://0funct0ry.github.io',
  base: '/microterm',
  integrations: [
    starlight({
      title: 'microterm',
      description: 'A minimal GPU-accelerated terminal emulator for macOS and Linux.',
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/0funct0ry/microterm' }],
      sidebar: [
        { label: 'Getting started', items: [{ label: 'Installation', slug: 'docs/installation' }] },
      ],
    }),
  ],
});
