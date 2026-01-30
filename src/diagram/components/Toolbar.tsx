import * as React from 'react';
import clsx from 'clsx';

import styles from './Toolbar.module.css';

export interface ToolbarProps {
  variant?: 'dense' | 'regular';
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function Toolbar(props: ToolbarProps): React.ReactElement {
  const { variant = 'regular', className, style, children } = props;

  return (
    <div className={clsx(styles.toolbar, variant === 'dense' ? styles.dense : styles.regular, className)} style={style}>
      {children}
    </div>
  );
}
