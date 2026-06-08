// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core

import * as React from 'react';

import IconButton from './components/IconButton';
import { ArrowBackIcon, ChevronRightIcon, MenuIcon, SettingsIcon } from './components/icons';
import { breadcrumbSegments, isStdlibModel } from './module-navigation';
import type { ModuleStackEntry } from './module-navigation';

import styles from './Editor.module.css';

interface BreadcrumbBarProps {
  readonly modelStack: ReadonlyArray<ModuleStackEntry>;
  readonly modelName: string;
  readonly onBack: () => void;
  readonly onNavigateToLevel: (level: number) => void;
  readonly onShowDrawer: () => void;
}

/**
 * Renders the left portion of the search bar: either the hamburger menu
 * (at root level) or back arrow + settings + breadcrumb trail (when
 * inside a module). Extracted as a pure component for testability.
 */
export function BreadcrumbBar(props: BreadcrumbBarProps): React.ReactElement {
  const { modelStack, modelName, onBack, onNavigateToLevel, onShowDrawer } = props;
  const isNested = modelStack.length > 0;

  if (!isNested) {
    return (
      <IconButton className={styles.menuButton} aria-label="Menu" onClick={onShowDrawer} size="small">
        <MenuIcon />
      </IconButton>
    );
  }

  const segments = breadcrumbSegments(modelStack);
  const isStdlib = isStdlibModel(modelName);

  return (
    <>
      <IconButton className={styles.menuButton} aria-label="Back" onClick={onBack} size="small">
        <ArrowBackIcon />
      </IconButton>
      <IconButton className={styles.menuButton} aria-label="Model Properties" onClick={onShowDrawer} size="small">
        <SettingsIcon />
      </IconButton>
      <div className={styles.breadcrumb}>
        {segments.map((seg, i) => {
          const isCurrent = seg.level === modelStack.length;
          return (
            <React.Fragment key={seg.level}>
              {i > 0 && <ChevronRightIcon className={styles.breadcrumbSeparator} />}
              {isCurrent ? (
                <span className={styles.breadcrumbCurrent}>{seg.label}</span>
              ) : (
                <button className={styles.breadcrumbLink} onClick={() => onNavigateToLevel(seg.level)}>
                  {seg.label}
                </button>
              )}
            </React.Fragment>
          );
        })}
        {isStdlib && <span className={styles.readOnlyBadge}>read-only</span>}
      </div>
    </>
  );
}
