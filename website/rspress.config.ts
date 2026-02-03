import { defineConfig } from 'rspress/config';
import path from 'path';

const GithubBase = 'https://github.com/bpowers/simlin';

export default defineConfig({
  root: path.join(__dirname, 'docs'),
  title: 'Simlin',
  description: 'Debug your intuition',
  icon: '/img/favicon.ico',
  logo: {
    light: '/img/logo.svg',
    dark: '/img/logo.svg',
  },
  globalStyles: path.join(__dirname, 'src/css/custom.css'),
  themeConfig: {
    enableContentAnimation: true,
    footer: {
      message: '© Bobby Powers',
    },
    socialLinks: [
      {
        icon: 'github',
        mode: 'link',
        content: GithubBase,
      },
    ],
    nav: [
      {
        text: 'App',
        link: 'https://app.simlin.com',
      },
      {
        text: 'Docs',
        link: '/guide/',
      },
      {
        text: 'Blog',
        link: '/blog/',
      },
    ],
    sidebar: {
      '/guide/': [
        {
          text: 'Getting Started',
          items: [
            {
              text: 'Introduction',
              link: '/guide/',
            },
            {
              text: 'First Model',
              link: '/guide/first-model',
            },
            {
              text: 'Simlin App',
              link: '/guide/simlin-app',
            },
            {
              text: 'Cheat Sheet',
              link: '/guide/cheat-sheet',
            },
          ],
        },
      ],
    },
    editLink: {
      docRepoBaseUrl: `${GithubBase}/edit/main/website`,
      text: 'Edit this page on GitHub',
    },
  },
  builderConfig: {
    resolve: {
      alias: {
        '@simlin/core': path.resolve(__dirname, '../src/core'),
        '@simlin/diagram': path.resolve(__dirname, '../src/diagram'),
        '@simlin/engine': path.resolve(__dirname, '../src/engine'),
        '@': path.resolve(__dirname, 'src'),
      },
    },
    tools: {
      rspack: {
        experiments: {
          asyncWebAssembly: true,
        },
        module: {
          rules: [
            {
              test: /\.wasm$/,
              type: 'webassembly/async',
            },
          ],
        },
      },
    },
  },
  // Plugins
  plugins: [],
  // Routes for non-doc pages
  route: {
    include: ['**/*.tsx', '**/*.md', '**/*.mdx'],
  },
  // Output configuration
  outDir: 'build',
  // Analytics
  analytics: {
    ga: {
      measurementId: 'G-DYC89XS4YM',
    },
  },
  // Generate sitemap
  ssg: true,
});
