// @ts-check
import { defineConfig } from 'astro/config';
import { unified } from '@astrojs/markdown-remark';
import starlight from '@astrojs/starlight';
import starlightLlmsTxt from 'starlight-llms-txt';
import omitUnreleasedChangelog from './plugins/omit-unreleased-changelog.mjs';

export default defineConfig({
  site: 'https://aventasks.dev',
  markdown: {
    processor: unified({ remarkPlugins: [omitUnreleasedChangelog] }),
  },
  redirects: {
    '/workflows': '/getting-started/#next-steps',
  },
  integrations: [
    starlight({
      title: 'aven',
      description: 'A local-first task manager for power users and agents.',
      plugins: [starlightLlmsTxt()],
      logo: {
        dark: './src/assets/aven-landing-wordmark.svg',
        light: './src/assets/aven-wordmark-grape-violet-light.svg',
        alt: 'Aven logo',
        replacesTitle: true,
      },
      favicon: '/favicon.svg',
      expressiveCode: {
        themes: ['vesper', 'github-light'],
        useStarlightUiThemeColors: true,
        styleOverrides: {
          codeFontFamily: "'IBM Plex Mono', ui-monospace, monospace",
          uiFontFamily: "'Space Grotesk', system-ui, sans-serif",
          codeFontSize: '0.9rem',
          codeLineHeight: '1.6',
          codePaddingBlock: '0.8rem',
          codePaddingInline: '0.95rem',
          borderRadius: '10px',
          borderWidth: '1px',
          borderColor: ({ theme }) => (theme.type === 'dark' ? '#4b3a66' : '#e4dfd7'),
          frames: {
            editorBackground: ({ theme }) => (theme.type === 'dark' ? '#171620' : '#f7f5f1'),
            terminalBackground: ({ theme }) => (theme.type === 'dark' ? '#171620' : '#f7f5f1'),
            frameBoxShadowCssValue: 'none',
          },
        },
      },
      head: [
        {
          tag: 'meta',
          attrs: {
            name: 'theme-color',
            content: '#121110',
          },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'preload',
            href: '/fonts/space-grotesk-latin.woff2',
            as: 'font',
            type: 'font/woff2',
            crossorigin: true,
          },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'preload',
            href: '/fonts/ibm-plex-mono-latin.woff2',
            as: 'font',
            type: 'font/woff2',
            crossorigin: true,
          },
        },
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
        ThemeProvider: './src/components/ThemeProvider.astro',
      },
      customCss: [
        './src/styles/fonts.css',
        './src/styles/tokens.css',
        './src/styles/docs.css',
        './src/styles/code.css',
      ],
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'What is aven?', slug: 'overview' },
            { label: 'Getting started', slug: 'getting-started' },
            { label: 'Concepts', slug: 'concepts' },
            { label: 'Coming from Taskwarrior', slug: 'taskwarrior' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { label: 'Using the TUI', slug: 'tui' },
            { label: 'Organizing tasks', slug: 'organize-tasks' },
            { label: 'Scheduling tasks', slug: 'schedule-tasks' },
            { label: 'Recurring tasks', slug: 'recurring-tasks' },
            { label: 'Work with agents', slug: 'agents' },
            { label: 'Sync across devices', slug: 'sync' },
            { label: 'Back up and restore', slug: 'backups' },
            { label: 'Task metadata', slug: 'task-metadata' },
            { label: 'Custom TUI commands', slug: 'custom-commands' },
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
        {
          label: 'Community',
          items: [
            { label: 'Community projects', slug: 'community' },
          ],
        },
      ],
    }),
  ],
});
