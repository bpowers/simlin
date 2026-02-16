// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import * as RadixCheckbox from '@radix-ui/react-checkbox';
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

  return (
    <RadixCheckbox.Root
      className={clsx(styles.checkbox, color === 'secondary' ? styles.secondary : styles.primary, className)}
      style={style}
      checked={checked}
      defaultChecked={defaultChecked}
      onCheckedChange={onChange}
      disabled={disabled}
      name={name}
    >
      <RadixCheckbox.Indicator className={styles.indicator}>
        <CheckIcon className={styles.checkIcon} />
      </RadixCheckbox.Indicator>
    </RadixCheckbox.Root>
  );
}
