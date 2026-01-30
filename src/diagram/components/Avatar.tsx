import * as React from 'react';
import clsx from 'clsx';

import styles from './Avatar.module.css';

export interface AvatarProps {
  src?: string;
  alt?: string;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function Avatar(props: AvatarProps): React.ReactElement {
  const { src, alt, className, style, children } = props;

  return (
    <div className={clsx(styles.avatar, className)} style={style}>
      {src ? <img src={src} alt={alt || ''} className={styles.image} /> : children}
    </div>
  );
}
