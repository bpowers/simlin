// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import * as RadixAccordion from '@radix-ui/react-accordion';
import clsx from 'clsx';

import styles from './Accordion.module.css';

export interface AccordionProps {
  defaultExpanded?: boolean;
  expanded?: boolean;
  onChange?: (expanded: boolean) => void;
  disabled?: boolean;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function Accordion(props: AccordionProps): React.ReactElement {
  const { defaultExpanded, expanded, onChange, disabled, className, style, children } = props;

  const value = expanded ? 'item' : '';
  const defaultValue = defaultExpanded ? 'item' : undefined;

  return (
    <RadixAccordion.Root
      type="single"
      collapsible
      value={expanded !== undefined ? value : undefined}
      defaultValue={defaultValue}
      onValueChange={(newValue) => {
        if (onChange) {
          onChange(newValue === 'item');
        }
      }}
      disabled={disabled}
      className={clsx(styles.accordion, className)}
      style={style}
    >
      <RadixAccordion.Item value="item" className={styles.item}>
        {children}
      </RadixAccordion.Item>
    </RadixAccordion.Root>
  );
}

export interface AccordionSummaryProps {
  expandIcon?: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function AccordionSummary(props: AccordionSummaryProps): React.ReactElement {
  const { expandIcon, className, style, children } = props;

  return (
    <RadixAccordion.Header className={styles.header}>
      <RadixAccordion.Trigger className={clsx(styles.trigger, className)} style={style}>
        <span className={styles.content}>{children}</span>
        {expandIcon && <span className={styles.expandIcon}>{expandIcon}</span>}
      </RadixAccordion.Trigger>
    </RadixAccordion.Header>
  );
}

export interface AccordionDetailsProps {
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function AccordionDetails(props: AccordionDetailsProps): React.ReactElement {
  const { className, style, children } = props;

  return (
    <RadixAccordion.Content className={clsx(styles.details, className)} style={style}>
      <div className={styles.detailsInner}>{children}</div>
    </RadixAccordion.Content>
  );
}
