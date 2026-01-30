import * as React from 'react';
import clsx from 'clsx';

import styles from './Paper.module.css';

export interface PaperProps {
  elevation?: number;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function Paper(props: PaperProps): React.ReactElement {
  const { elevation = 1, className, style, children } = props;

  const elevationClass = elevation === 0 ? styles.elevation0 : elevation <= 4 ? styles.elevation1 : styles.elevation2;

  return (
    <div className={clsx(styles.paper, elevationClass, className)} style={style}>
      {children}
    </div>
  );
}
