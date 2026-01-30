import * as React from 'react';
import clsx from 'clsx';

import styles from './Card.module.css';

export interface CardProps {
  variant?: 'outlined' | 'elevation';
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function Card(props: CardProps): React.ReactElement {
  const { variant = 'elevation', className, style, children } = props;

  return (
    <div
      className={clsx(styles.card, variant === 'outlined' ? styles.outlined : styles.elevation, className)}
      style={style}
    >
      {children}
    </div>
  );
}

export interface CardContentProps {
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function CardContent(props: CardContentProps): React.ReactElement {
  const { className, style, children } = props;

  return (
    <div className={clsx(styles.cardContent, className)} style={style}>
      {children}
    </div>
  );
}

export interface CardActionsProps {
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function CardActions(props: CardActionsProps): React.ReactElement {
  const { className, style, children } = props;

  return (
    <div className={clsx(styles.cardActions, className)} style={style}>
      {children}
    </div>
  );
}
