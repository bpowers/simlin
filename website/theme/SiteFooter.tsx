// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import React from 'react';
import { Link } from 'rspress/theme';

interface FooterColumn {
  title: string;
  items: Array<{ label: string; href: string; external?: boolean }>;
}

const columns: FooterColumn[] = [
  {
    title: 'Simlin',
    items: [
      { label: 'App', href: 'https://app.simlin.com', external: true },
      { label: 'Terms and Conditions', href: '/terms' },
      { label: 'Privacy Policy', href: '/privacy' },
    ],
  },
  {
    title: 'Docs',
    items: [{ label: 'Getting Started', href: '/docs' }],
  },
  {
    title: 'More',
    items: [
      { label: 'Blog', href: '/blog' },
      { label: 'GitHub', href: 'https://github.com/bpowers/simlin', external: true },
    ],
  },
];

// The dark, multi-column footer from the classic Docusaurus site, rendered
// on every page via the theme Layout's `bottom` slot.
export function SiteFooter(): React.ReactElement {
  return (
    <footer className="simlin-footer">
      <div className="simlin-container">
        <div className="simlin-footer-columns">
          {columns.map((column) => (
            <div className="simlin-footer-column" key={column.title}>
              <div className="simlin-footer-title">{column.title}</div>
              <ul className="simlin-footer-items">
                {column.items.map((item) => (
                  <li key={item.label}>
                    {item.external ? <a href={item.href}>{item.label}</a> : <Link href={item.href}>{item.label}</Link>}
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
        <div className="simlin-footer-copyright">© Bobby Powers</div>
      </div>
    </footer>
  );
}
