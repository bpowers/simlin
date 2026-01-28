// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import * as RadixTabs from '@radix-ui/react-tabs';
import clsx from 'clsx';

import styles from './Tabs.module.css';

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

export class Tab extends React.PureComponent<TabProps> {
  // value is injected by Tabs parent
  render() {
    const { label, ...rest } = this.props;
    const value = (rest as any)._tabValue as string;
    return (
      <RadixTabs.Trigger value={value} className={styles.tab}>
        {label}
      </RadixTabs.Trigger>
    );
  }
}

export class Tabs extends React.PureComponent<TabsProps> {
  handleValueChange = (newValue: string) => {
    const syntheticEvent = {} as React.SyntheticEvent;
    this.props.onChange(syntheticEvent, Number(newValue));
  };

  render() {
    const { className, value, children, ...rest } = this.props;
    const ariaLabel = rest['aria-label'];

    // Count children (non-null) and inject _tabValue
    const childArray = React.Children.toArray(children).filter(Boolean);
    const tabCount = childArray.length;

    const enrichedChildren = childArray.map((child, index) => {
      if (React.isValidElement(child)) {
        return React.cloneElement(child as React.ReactElement<any>, { _tabValue: String(index) });
      }
      return child;
    });

    const indicatorLeft = tabCount > 0 ? `${(value / tabCount) * 100}%` : '0%';
    const indicatorWidth = tabCount > 0 ? `${(1 / tabCount) * 100}%` : '0%';

    return (
      <RadixTabs.Root value={String(value)} onValueChange={this.handleValueChange}>
        <RadixTabs.List className={clsx(styles.tabsList, className)} aria-label={ariaLabel}>
          {enrichedChildren}
          <div
            className={styles.indicator}
            style={{ left: indicatorLeft, width: indicatorWidth }}
          />
        </RadixTabs.List>
      </RadixTabs.Root>
    );
  }
}
