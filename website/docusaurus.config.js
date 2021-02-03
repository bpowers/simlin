const GithubBase = 'https://github.com/bpowers/simlin';

module.exports = {
  title: 'Simlin',
  tagline: 'Debug your intuition',
  url: 'https://simlin.com',
  baseUrl: '/',
  onBrokenLinks: 'throw',
  onBrokenMarkdownLinks: 'throw',
  favicon: 'img/favicon.ico',
  organizationName: 'bpowers', // Usually your GitHub org/user name.
  projectName: 'simlin', // Usually your repo name.
  themeConfig: {
    navbar: {
      title: 'Simlin',
      logo: {
        alt: 'Simlin Logo',
        src: 'img/logo.svg',
      },
      items: [
        {to: 'https://app.simlin.com', label: 'App', position: 'left'},
        {
          to: 'docs/',
          activeBasePath: 'docs',
          label: 'Docs',
          position: 'left',
        },
        {to: 'blog', label: 'Blog', position: 'left'},
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Simlin',
          items: [
            {
              label: 'App',
              to: 'http://app.simlin.com',
            },
          ],
        },
        {
          title: 'Docs',
          items: [
            {
              label: 'Getting Started',
              to: 'docs/',
            },
          ],
        },
        // {
        //   title: 'Community',
        //   items: [
        //     {
        //       label: 'Stack Overflow',
        //       href: 'https://stackoverflow.com/questions/tagged/docusaurus',
        //     },
        //     {
        //       label: 'Discord',
        //       href: 'https://discordapp.com/invite/docusaurus',
        //     },
        //     {
        //       label: 'Twitter',
        //       href: 'https://twitter.com/docusaurus',
        //     },
        //   ],
        // },
        {
          title: 'More',
          items: [
            {
              label: 'Blog',
              to: 'blog',
            },
            {
              label: 'GitHub',
              href: GithubBase,
            },
          ],
        },
      ],
      copyright: `© Bobby Powers`,
    },
  },
  presets: [
    [
      '@docusaurus/preset-classic',
      {
        docs: {
          sidebarPath: require.resolve('./sidebars.js'),
          // Please change this to your repo.
          editUrl:
            GithubBase + '/edit/main/website',
        },
        blog: {
          showReadingTime: false,
          // Please change this to your repo.
          editUrl:
            GithubBase + '/edit/main/website/blog/',
        },
        theme: {
          customCss: require.resolve('./src/css/custom.css'),
        },
      },
    ],
  ],
};
