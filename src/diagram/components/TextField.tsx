// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import styles from './TextField.module.css';

interface TextFieldProps {
  variant?: 'outlined' | 'standard';
  label?: string;
  value?: string | number;
  onChange?: (event: React.ChangeEvent<HTMLInputElement>) => void;
  type?: string;
  margin?: 'none' | 'normal';
  fullWidth?: boolean;
  error?: boolean;
  placeholder?: string;
  className?: string;
  InputProps?: {
    disableUnderline?: boolean;
    ref?: React.Ref<HTMLDivElement>;
  };
  inputProps?: React.InputHTMLAttributes<HTMLInputElement>;
}

interface TextFieldState {
  isFocused: boolean;
}

export default class TextField extends React.PureComponent<TextFieldProps, TextFieldState> {
  state: TextFieldState = { isFocused: false };

  handleFocus = () => {
    this.setState({ isFocused: true });
  };

  handleBlur = () => {
    this.setState({ isFocused: false });
  };

  render() {
    const {
      variant = 'outlined',
      label,
      value,
      onChange,
      type,
      margin,
      fullWidth,
      error,
      placeholder,
      className,
      InputProps,
      inputProps,
      ...rest
    } = this.props;
    const { isFocused } = this.state;

    const hasValue = value !== undefined && value !== null && value !== '';
    const shouldShrink = isFocused || hasValue;

    const rootClasses = clsx(
      styles.root,
      fullWidth && styles.fullWidth,
      margin === 'normal' && styles.marginNormal,
      className,
    );

    if (variant === 'standard') {
      const disableUnderline = InputProps?.disableUnderline;
      const wrapperRef = InputProps?.ref;

      const wrapperClasses = clsx(
        styles.standardWrapper,
        disableUnderline && styles.standardNoUnderline,
        isFocused && !disableUnderline && styles.standardFocused,
        error && !disableUnderline && styles.standardError,
      );

      const labelClasses = label
        ? clsx(
            styles.standardLabel,
            shouldShrink && styles.standardLabelShrunk,
            isFocused && styles.standardLabelFocused,
            error && styles.standardLabelError,
          )
        : undefined;

      return (
        <div className={rootClasses}>
          <div className={wrapperClasses} ref={wrapperRef}>
            {label && <label className={labelClasses}>{label}</label>}
            <input
              className={styles.standardInput}
              value={value}
              onChange={onChange}
              type={type}
              placeholder={placeholder}
              onFocus={this.handleFocus}
              onBlur={this.handleBlur}
              {...inputProps}
              {...rest}
            />
          </div>
        </div>
      );
    }

    // outlined variant
    const wrapperClasses = clsx(
      styles.outlinedWrapper,
      isFocused && styles.outlinedFocused,
      error && styles.outlinedError,
    );

    const labelClasses = label
      ? clsx(
          styles.outlinedLabel,
          shouldShrink && styles.outlinedLabelShrunk,
          isFocused && styles.outlinedLabelFocused,
          error && styles.outlinedLabelError,
        )
      : undefined;

    return (
      <div className={rootClasses}>
        <div className={wrapperClasses}>
          {label && <label className={labelClasses}>{label}</label>}
          <input
            className={styles.outlinedInput}
            value={value}
            onChange={onChange}
            type={type}
            placeholder={placeholder}
            onFocus={this.handleFocus}
            onBlur={this.handleBlur}
            {...inputProps}
            {...rest}
          />
        </div>
      </div>
    );
  }
}
