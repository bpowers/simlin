// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

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
  logoText: 'Simlin',
  globalStyles: path.join(__dirname, 'src/css/custom.css'),
  themeConfig: {
    // The production site is light-only; hide the appearance toggle.
    darkMode: false,
    // Match the deployed site's navbar: no search box (GitHub lives in the
    // footer's More column instead of a nav icon).
    search: false,
    enableContentAnimation: true,
    nav: [
      {
        text: 'App',
        link: 'https://app.simlin.com',
      },
      {
        text: 'Docs',
        link: '/docs',
      },
      {
        text: 'Blog',
        link: '/blog',
      },
    ],
    sidebar: {
      '/docs': [
        {
          text: 'Getting Started',
          items: [
            {
              text: 'The Simlin App',
              link: '/docs',
            },
            {
              text: 'Your First Model',
              link: '/docs/first-model',
            },
            {
              text: 'Editor Cheat Sheet',
              link: '/docs/cheat-sheet',
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
    html: {
      // Google Analytics. Rspress has no first-class analytics option, so
      // inject the gtag snippet directly into every page.
      tags: [
        {
          tag: 'script',
          attrs: { async: true, src: 'https://www.googletagmanager.com/gtag/js?id=G-DYC89XS4YM' },
        },
        {
          tag: 'script',
          children:
            "window.dataLayer=window.dataLayer||[];function gtag(){dataLayer.push(arguments);}gtag('js',new Date());gtag('config','G-DYC89XS4YM');",
        },
      ],
    },
    tools: {
      bundlerChain(chain, { CHAIN_ID }) {
        // Keep the Node-build CSS stubs out of the CSS pipeline (see the
        // javascript/auto rule below for why they exist).
        chain.module.rule(CHAIN_ID.RULE.CSS).exclude.add(/diagram[\\/]lib[\\/].*\.css$/);
      },
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
            // @simlin/diagram's Node build (lib/) stubs its CSS files with
            // CommonJS shims so Node can require() them; only the browser
            // build (lib.browser/) carries real CSS. The SSG bundle resolves
            // the Node build, so parse those stubs as JavaScript instead of
            // feeding them to the CSS pipeline. The web bundle never sees
            // lib/ (it resolves the `browser` condition), so this rule is
            // inert there.
            {
              test: /diagram[\\/]lib[\\/].*\.css$/,
              type: 'javascript/auto',
            },
          ],
        },
      },
    },
  },
  // Plugins
  plugins: [],
  // Extensionless links, matching the URLs the site has always served
  // (GitHub Pages resolves /terms to terms.html).
  route: {
    cleanUrls: true,
  },
  // Output configuration
  outDir: 'build',
  // Generate sitemap
  ssg: true,
});
