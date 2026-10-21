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
        {
          label: 'Getting started',
          items: [
            { label: 'Installation', slug: 'docs/installation' },
            { label: 'Quick start', slug: 'docs/quick-start' },
            { label: 'Tabs & workspaces', slug: 'docs/tabs-and-workspaces' },
            { label: 'Panes & splits', slug: 'docs/panes-and-splits' },
            { label: 'Command palette & launcher', slug: 'docs/command-palette-and-launcher' },
            { label: 'Mouse & selection', slug: 'docs/mouse-and-selection' },
            { label: 'Hyperlinks', slug: 'docs/hyperlinks' },
            { label: 'Quick select', slug: 'docs/quick-select' },
            { label: 'Copy mode', slug: 'docs/copy-mode' },
            { label: 'Search', slug: 'docs/search' },
            { label: 'SSH domains', slug: 'docs/ssh-domains' },
            { label: 'microterm-mux', slug: 'docs/microterm-mux' },
          ],
        },
        {
          label: 'Automation',
          items: [
            { label: 'CLI overview', slug: 'docs/cli-overview' },
            { label: 'microterm ctl', slug: 'docs/ctl' },
            { label: 'Interactive shell', slug: 'docs/sh' },
            { label: 'Selectors & targeting', slug: 'docs/selectors-and-targeting' },
            { label: 'Run & wait', slug: 'docs/run-and-wait' },
            { label: 'Notifications & status', slug: 'docs/notifications-and-status' },
            { label: 'Shell integration', slug: 'docs/shell-integration' },
            { label: 'Recording & replay', slug: 'docs/recording-and-replay' },
            { label: 'Agents & automation', slug: 'docs/agents-and-automation' },
          ],
        },
        {
          label: 'Customising',
          items: [
            { label: 'Config file', slug: 'docs/config-file' },
            { label: 'Hot reload', slug: 'docs/hot-reload' },
            { label: 'Themes', slug: 'docs/themes' },
            { label: 'Fonts', slug: 'docs/fonts' },
            { label: 'Context menus', slug: 'docs/context-menus' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Actions', slug: 'docs/reference/actions' },
            { label: 'Context menus', slug: 'docs/reference/context-menus' },
            { label: 'Agent skill', slug: 'docs/reference/agent-skill' },
            { label: 'Terminal API (VT) support', slug: 'docs/reference/terminal-api' },
            { label: 'Environment variables', slug: 'docs/reference/environment-variables' },
            { label: 'Exit codes', slug: 'docs/reference/exit-codes' },
          ],
        },
      ],
    }),
  ],
});
