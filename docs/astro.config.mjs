// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://aven.raine.dev',
  integrations: [
    starlight({
      title: 'aven',
      description: 'A local-first task manager for power users and agents.',
      logo: {
        dark: './src/assets/aven-logo-grape-violet.svg',
        light: './src/assets/aven-logo-grape-violet-light.svg',
        alt: 'Aven logo',
      },
      favicon: '/favicon.svg',
      head: [
        {
          tag: 'script',
          attrs: {
            src: '/image-zoom.js',
            defer: true,
          },
        },
      ],
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/raine/aven' },
      ],
      customCss: ['./src/styles/code.css'],
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'What is aven?', link: '/' },
            { label: 'Getting started', slug: 'getting-started' },
            { label: 'Concepts', slug: 'concepts' },
            { label: 'Taskwarrior comparison', slug: 'taskwarrior' },
          ],
        },
        {
          label: 'Using aven',
          items: [
            { label: 'TUI', slug: 'tui' },
            { label: 'Agents', slug: 'agents' },
            { label: 'Sync and backups', slug: 'sync' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Configuration', slug: 'configuration' },
          ],
        },
      ],
    }),
  ],
});
