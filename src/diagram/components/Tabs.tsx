// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import * as RadixTabs from '@radix-ui/react-tabs';
import clsx from 'clsx';

import styles from './Tabs.module.css';

// Context for passing tab index from parent to child
const TabIndexContext = React.createContext<string>('0');

interface TabsProps {
  className?: string;
  variant?: 'fullWidth';
  value: number;
  indicatorColor?: string;
  textColor?: string;
  onChange: (event: React.SyntheticEvent, newValue: number) => void;
  'aria-label'?: string;
  children?: React.ReactNode;
}

interface TabProps {
  label: string;
}

export function Tab(props: TabProps): React.ReactElement {
  const { label } = props;
  // The tab's Radix value is its zero-based index, injected by the parent Tabs
  // via context (so an individual <Tab> doesn't need to know its position).
  const tabValue = React.useContext(TabIndexContext);
  return (
    <RadixTabs.Trigger value={tabValue} className={styles.tab}>
      {label}
    </RadixTabs.Trigger>
  );
}

export function Tabs(props: TabsProps): React.ReactElement {
  const { className, value, onChange, children, ...rest } = props;
  const ariaLabel = rest['aria-label'];

  const handleValueChange = (newValue: string): void => {
    const syntheticEvent = {} as React.SyntheticEvent;
    onChange(syntheticEvent, Number(newValue));
  };

  // Count children (non-null) and wrap each with context provider
  const childArray = React.Children.toArray(children).filter(Boolean);
  const tabCount = childArray.length;

  const enrichedChildren = childArray.map((child, index) => (
    <TabIndexContext.Provider key={index} value={String(index)}>
      {child}
    </TabIndexContext.Provider>
  ));

  const indicatorLeft = tabCount > 0 ? `${(value / tabCount) * 100}%` : '0%';
  const indicatorWidth = tabCount > 0 ? `${(1 / tabCount) * 100}%` : '0%';

  return (
    <RadixTabs.Root value={String(value)} onValueChange={handleValueChange}>
      <RadixTabs.List className={clsx(styles.tabsList, className)} aria-label={ariaLabel}>
        {enrichedChildren}
        <div className={styles.indicator} style={{ left: indicatorLeft, width: indicatorWidth }} />
      </RadixTabs.List>
    </RadixTabs.Root>
  );
}
