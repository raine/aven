// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://aven.raine.dev',
  integrations: [
    starlight({
      title: 'aven',
      description: 'Local-first task management for humans and coding agents.',
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/raine/aven' },
      ],
      customCss: ['./src/styles/code.css'],
      sidebar: [
        { label: 'What is aven?', link: '/' },
        { label: 'Getting started', slug: 'getting-started' },
        { label: 'Concepts', slug: 'concepts' },
        { label: 'Configuration', slug: 'configuration' },
        { label: 'TUI', slug: 'tui' },
        { label: 'Agents', slug: 'agents' },
        { label: 'Sync and backups', slug: 'sync' },
      ],
    }),
  ],
});
