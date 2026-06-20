// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import clsx from 'clsx';

import styles from './Accordion.module.css';

interface AccordionContextValue {
  open: boolean;
  disabled: boolean;
  toggle: () => void;
  triggerId: string;
  contentId: string;
}

// Shares the single-item disclosure state between the composed
// Accordion/AccordionSummary/AccordionDetails parts, the way Radix's internal
// context did, so callers keep composing them as independent children.
const AccordionContext = React.createContext<AccordionContextValue | undefined>(undefined);

function useAccordionContext(part: string): AccordionContextValue {
  const ctx = React.useContext(AccordionContext);
  if (!ctx) {
    throw new Error(`${part} must be rendered inside an <Accordion>`);
  }
  return ctx;
}

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
  const { defaultExpanded, expanded, onChange, disabled = false, className, style, children } = props;

  const isControlled = expanded !== undefined;
  const [internalOpen, setInternalOpen] = React.useState(defaultExpanded ?? false);
  const open = isControlled ? expanded : internalOpen;

  const reactId = React.useId();
  const triggerId = `${reactId}-trigger`;
  const contentId = `${reactId}-content`;

  const toggle = React.useCallback(() => {
    if (disabled) {
      return;
    }
    const next = !open;
    if (!isControlled) {
      setInternalOpen(next);
    }
    if (onChange) {
      onChange(next);
    }
  }, [disabled, open, isControlled, onChange]);

  const ctx = React.useMemo<AccordionContextValue>(
    () => ({ open, disabled, toggle, triggerId, contentId }),
    [open, disabled, toggle, triggerId, contentId],
  );

  return (
    <div className={clsx(styles.accordion, className)} style={style} data-state={open ? 'open' : 'closed'}>
      <AccordionContext.Provider value={ctx}>
        <div className={styles.item}>{children}</div>
      </AccordionContext.Provider>
    </div>
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
  const { open, disabled, toggle, triggerId, contentId } = useAccordionContext('AccordionSummary');

  return (
    <h3 className={styles.header}>
      <button
        type="button"
        id={triggerId}
        className={clsx(styles.trigger, className)}
        style={style}
        aria-expanded={open}
        aria-controls={contentId}
        disabled={disabled}
        data-state={open ? 'open' : 'closed'}
        data-disabled={disabled ? '' : undefined}
        onClick={toggle}
      >
        <span className={styles.content}>{children}</span>
        {expandIcon && <span className={styles.expandIcon}>{expandIcon}</span>}
      </button>
    </h3>
  );
}

export interface AccordionDetailsProps {
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function AccordionDetails(props: AccordionDetailsProps): React.ReactElement {
  const { className, style, children } = props;
  const { open, triggerId, contentId } = useAccordionContext('AccordionDetails');

  return (
    <div
      id={contentId}
      role="region"
      aria-labelledby={triggerId}
      className={clsx(styles.details, className)}
      style={style}
      data-state={open ? 'open' : 'closed'}
    >
      <div className={styles.detailsInner}>
        <div className={styles.detailsContent}>{children}</div>
      </div>
    </div>
  );
}
