// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import React from 'react';
import Theme from 'rspress/theme';

import { HomePage } from './HomePage';
import { SiteFooter } from './SiteFooter';

// The default Layout resolves HomeLayout through this module (the `@theme`
// virtual module), so exporting our HomePage as HomeLayout replaces the stock
// hero/features home page; the `bottom` slot renders the footer site-wide.
const Layout = (): React.ReactElement => <Theme.Layout bottom={<SiteFooter />} />;

export default {
  ...Theme,
  Layout,
  HomeLayout: HomePage,
};

export * from 'rspress/theme';
