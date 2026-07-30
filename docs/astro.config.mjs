// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLlmsTxt from 'starlight-llms-txt';

export default defineConfig({
  site: 'https://aven.raine.dev',
  redirects: {
    '/workflows': '/getting-started/#next-steps',
  },
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
            { label: 'Coming from Taskwarrior', slug: 'taskwarrior' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { label: 'Using the TUI', slug: 'tui' },
            { label: 'Organize tasks', slug: 'organize-tasks' },
            { label: 'Schedule tasks', slug: 'schedule-tasks' },
            { label: 'Recurring tasks', slug: 'recurring-tasks' },
            { label: 'Work with agents', slug: 'agents' },
            { label: 'Sync across devices', slug: 'sync' },
            { label: 'Back up and restore', slug: 'backups' },
            { label: 'Tips', slug: 'tips' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Command reference', slug: 'command-reference' },
            { label: 'Configuration', slug: 'configuration' },
            { label: 'Changelog', slug: 'changelog' },
          ],
        },
      ],
    }),
  ],
});
