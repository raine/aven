// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLlmsTxt from 'starlight-llms-txt';

export default defineConfig({
  site: 'https://aven.raine.dev',
  integrations: [
    starlight({
      title: 'aven',
      description: 'A local-first task manager for power users and agents.',
      plugins: [starlightLlmsTxt()],
      logo: {
        dark: './src/assets/aven-wordmark-grape-violet.svg',
        light: './src/assets/aven-wordmark-grape-violet-light.svg',
        alt: 'Aven logo',
        replacesTitle: true,
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
        {
          tag: 'script',
          attrs: {
            src: '/video-player.js',
            defer: true,
          },
        },
      ],
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/raine/aven' },
      ],
      components: {
        SocialIcons: './src/components/HeaderLinks.astro',
      },
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
            { label: 'Workflows', slug: 'workflows' },
            { label: 'Agents', slug: 'agents' },
            { label: 'Sync and backups', slug: 'sync' },
            { label: 'Tips', slug: 'tips' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Configuration', slug: 'configuration' },
            { label: 'Changelog', slug: 'changelog' },
          ],
        },
      ],
    }),
  ],
});
