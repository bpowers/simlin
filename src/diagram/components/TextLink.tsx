import * as React from 'react';
import clsx from 'clsx';

import styles from './TextLink.module.css';

export interface TextLinkProps {
  href?: string;
  underline?: 'none' | 'hover' | 'always';
  onClick?: (event: React.MouseEvent) => void;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function TextLink(props: TextLinkProps): React.ReactElement {
  const { href, underline = 'always', onClick, className, style, children } = props;

  const underlineClass =
    underline === 'none'
      ? styles.underlineNone
      : underline === 'hover'
        ? styles.underlineHover
        : styles.underlineAlways;

  const handleClick = (event: React.MouseEvent<HTMLAnchorElement>) => {
    if (onClick) {
      onClick(event);
    }
  };

  return (
    <a href={href} onClick={handleClick} className={clsx(styles.link, underlineClass, className)} style={style}>
      {children}
    </a>
  );
}
