import * as React from 'react';
import clsx from 'clsx';

import styles from './AppBar.module.css';

export interface AppBarProps {
  position?: 'fixed' | 'static' | 'sticky';
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function AppBar(props: AppBarProps): React.ReactElement {
  const { position = 'static', className, style, children } = props;

  const positionClass =
    position === 'fixed' ? styles.positionFixed : position === 'sticky' ? styles.positionSticky : styles.positionStatic;

  return (
    <header className={clsx(styles.appBar, positionClass, className)} style={style}>
      {children}
    </header>
  );
}
