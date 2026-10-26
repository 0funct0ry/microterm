import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { readFileSync } from 'node:fs';

// The workspace version from ../Cargo.toml, shown in the header badge and install snippets.
const cargo = readFileSync(new URL('../Cargo.toml', import.meta.url), 'utf8');
const VERSION = cargo.match(/\[workspace\.package\][^[]*?version\s*=\s*"([^"]+)"/)?.[1] ?? '1.0.0';

// Content links are written root-relative (`/docs/themes/`). The site lives under a base path, so
// prefix them at build time instead of hard-coding the base in every page.
const BASE = '/microterm';
function rehypeBaseLinks() {
  const walk = (node) => {
    if (node.type === 'element' && node.tagName === 'a') {
      const href = node.properties?.href;
      if (typeof href === 'string' && href.startsWith('/docs/')) node.properties.href = BASE + href;
    }
    (node.children ?? []).forEach(walk);
  };
  return walk;
}

// Deployed to https://0funct0ry.github.io/microterm/ (SPEC §16). The docs sidebar follows the
// information architecture in SPEC §16; Pagefind search is Starlight's built-in.
export default defineConfig({
  site: 'https://0funct0ry.github.io',
  base: BASE,
  markdown: { rehypePlugins: [rehypeBaseLinks] },
  vite: { define: { __MT_VERSION__: JSON.stringify(VERSION) } },
  integrations: [
    starlight({
      title: 'microterm',
      description: 'A minimal GPU-accelerated terminal emulator for macOS and Linux.',
      logo: { src: './src/assets/logo.svg', alt: 'microterm' },
      favicon: '/favicon.svg',
      head: [
        { tag: 'link', attrs: { rel: 'icon', href: '/microterm/favicon.ico', sizes: '48x48' } },
        { tag: 'link', attrs: { rel: 'apple-touch-icon', href: '/microterm/apple-touch-icon.png' } },
        { tag: 'meta', attrs: { property: 'og:image', content: 'https://0funct0ry.github.io/microterm/og.png' } },
      ],
      customCss: ['./src/styles/fonts.css', './src/styles/tokens.css', './src/styles/docs.css'],
      components: {
        SiteTitle: './src/components/SiteTitle.astro',
        Footer: './src/components/Footer.astro',
      },
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/0funct0ry/microterm' }],
      editLink: { baseUrl: 'https://github.com/0funct0ry/microterm/edit/main/site/' },
      sidebar: [
        {
          label: 'Getting started',
          items: [
            { label: 'Installation', slug: 'docs/installation' },
            { label: 'Quick start', slug: 'docs/quick-start' },
            { label: 'Upgrading', slug: 'docs/upgrading' },
          ],
        },
        {
          label: 'Configuration',
          items: [
            { label: 'Config file', slug: 'docs/config-file' },
            { label: 'Hot reload', slug: 'docs/hot-reload' },
            { label: 'Fonts', slug: 'docs/fonts' },
            { label: 'Themes', slug: 'docs/themes' },
            { label: 'Key bindings', slug: 'docs/key-bindings' },
            { label: 'Reference (generated)', slug: 'docs/reference/configuration' },
          ],
        },
        {
          label: 'Features',
          items: [
            { label: 'Tabs & workspaces', slug: 'docs/tabs-and-workspaces' },
            { label: 'Panes & splits', slug: 'docs/panes-and-splits' },
            { label: 'Command palette & launcher', slug: 'docs/command-palette-and-launcher' },
            { label: 'Menu bar', slug: 'docs/menu-bar' },
            { label: 'Context menus', slug: 'docs/context-menus' },
            { label: 'Quick select', slug: 'docs/quick-select' },
            { label: 'Copy mode', slug: 'docs/copy-mode' },
            { label: 'Search', slug: 'docs/search' },
            { label: 'Mouse & selection', slug: 'docs/mouse-and-selection' },
            { label: 'Hyperlinks', slug: 'docs/hyperlinks' },
            { label: 'SSH domains', slug: 'docs/ssh-domains' },
            { label: 'Serial ports', slug: 'docs/serial-ports' },
            { label: 'Snippets', slug: 'docs/snippets' },
            { label: 'Triggers', slug: 'docs/triggers' },
            { label: 'Shell integration', slug: 'docs/shell-integration' },
            { label: 'Private OSC', slug: 'docs/private-osc' },
          ],
        },
        {
          label: 'CLI',
          items: [
            { label: 'Overview', slug: 'docs/cli-overview' },
            { label: 'microterm ctl', slug: 'docs/ctl' },
            { label: 'Interactive shell', slug: 'docs/sh' },
            { label: 'Selectors & targeting', slug: 'docs/selectors-and-targeting' },
            { label: 'Run & wait', slug: 'docs/run-and-wait' },
            { label: 'Notifications & status', slug: 'docs/notifications-and-status' },
            { label: 'Agents & automation', slug: 'docs/agents-and-automation' },
            { label: 'Recording & replay', slug: 'docs/recording-and-replay' },
            { label: 'Exporting recordings', slug: 'docs/exporting-recordings' },
            { label: 'microterm-mux', slug: 'docs/microterm-mux' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Actions', slug: 'docs/reference/actions' },
            { label: 'Template functions', slug: 'docs/reference/template-functions' },
            { label: 'Context menus', slug: 'docs/reference/context-menus' },
            { label: 'Export formats', slug: 'docs/reference/export-formats' },
            { label: 'Agent skill', slug: 'docs/reference/agent-skill' },
            { label: 'Escape sequences', slug: 'docs/reference/terminal-api' },
            { label: 'Environment variables', slug: 'docs/reference/environment-variables' },
            { label: 'Exit codes', slug: 'docs/reference/exit-codes' },
          ],
        },
        {
          label: 'Project',
          items: [
            { label: 'Roadmap', slug: 'docs/roadmap' },
            { label: 'Changelog', slug: 'docs/changelog' },
            { label: 'Contributing', slug: 'docs/contributing' },
            { label: 'Licences', slug: 'docs/licenses' },
          ],
        },
      ],
    }),
  ],
});
