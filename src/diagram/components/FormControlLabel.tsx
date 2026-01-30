import * as React from 'react';
import clsx from 'clsx';

import styles from './FormControlLabel.module.css';

export interface FormControlLabelProps {
  control: React.ReactElement;
  label: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}

export default function FormControlLabel(props: FormControlLabelProps): React.ReactElement {
  const { control, label, className, style } = props;

  return (
    <label className={clsx(styles.formControlLabel, className)} style={style}>
      {control}
      <span className={styles.label}>{label}</span>
    </label>
  );
}
