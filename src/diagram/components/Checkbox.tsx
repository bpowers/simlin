// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import clsx from 'clsx';

import styles from './Checkbox.module.css';
import { CheckIcon } from './icons';

export interface CheckboxProps {
  checked?: boolean;
  defaultChecked?: boolean;
  onChange?: (checked: boolean) => void;
  disabled?: boolean;
  name?: string;
  color?: 'primary' | 'secondary';
  className?: string;
  style?: React.CSSProperties;
}

export default function Checkbox(props: CheckboxProps): React.ReactElement {
  const { checked, defaultChecked, onChange, disabled, name, color = 'primary', className, style } = props;

  // Support both controlled (`checked`) and uncontrolled (`defaultChecked`)
  // use, mirroring the prior Radix behavior so the rendered data-state always
  // reflects the live value rather than only the initial prop.
  const isControlled = checked !== undefined;
  const [internalChecked, setInternalChecked] = React.useState(defaultChecked ?? false);
  const isChecked = isControlled ? checked : internalChecked;

  const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const next = event.target.checked;
    if (!isControlled) {
      setInternalChecked(next);
    }
    if (onChange) {
      onChange(next);
    }
  };

  return (
    <span className={styles.root}>
      <input
        type="checkbox"
        className={clsx(styles.checkbox, color === 'secondary' ? styles.secondary : styles.primary, className)}
        style={style}
        data-state={isChecked ? 'checked' : 'unchecked'}
        checked={isChecked}
        onChange={handleChange}
        disabled={disabled}
        name={name}
      />
      {isChecked && (
        <span className={styles.indicator} aria-hidden="true">
          <CheckIcon className={styles.checkIcon} />
        </span>
      )}
    </span>
  );
}
