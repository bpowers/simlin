// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import styles from './TextField.module.css';

let textFieldIdCounter = 0;

interface TextFieldProps {
  id?: string;
  variant?: 'outlined' | 'standard';
  label?: string;
  value?: string | number;
  onChange?: (event: React.ChangeEvent<HTMLInputElement>) => void;
  type?: string;
  margin?: 'none' | 'normal' | 'dense';
  fullWidth?: boolean;
  error?: boolean;
  helperText?: React.ReactNode;
  placeholder?: string;
  className?: string;
  autoFocus?: boolean;
  autoComplete?: string;
  name?: string;
  InputProps?: {
    disableUnderline?: boolean;
    ref?: React.Ref<HTMLDivElement>;
    startAdornment?: React.ReactNode;
  };
  inputProps?: React.InputHTMLAttributes<HTMLInputElement>;
  onKeyPress?: (event: React.KeyboardEvent<HTMLInputElement>) => void;
}

interface TextFieldState {
  isFocused: boolean;
  generatedId: string;
}

export default class TextField extends React.PureComponent<TextFieldProps, TextFieldState> {
  constructor(props: TextFieldProps) {
    super(props);
    this.state = {
      isFocused: false,
      generatedId: `textfield-${++textFieldIdCounter}`,
    };
  }

  handleFocus = (event: React.FocusEvent<HTMLInputElement>) => {
    this.setState({ isFocused: true });
    // Chain with any external handler from inputProps
    this.props.inputProps?.onFocus?.(event);
  };

  handleBlur = (event: React.FocusEvent<HTMLInputElement>) => {
    this.setState({ isFocused: false });
    // Chain with any external handler from inputProps
    this.props.inputProps?.onBlur?.(event);
  };

  render() {
    const {
      id,
      variant = 'outlined',
      label,
      value,
      onChange,
      type,
      margin,
      fullWidth,
      error,
      helperText,
      placeholder,
      className,
      autoFocus,
      autoComplete,
      name,
      InputProps,
      inputProps,
      onKeyPress,
      ...rest
    } = this.props;
    const { isFocused, generatedId } = this.state;

    const inputId = id || generatedId;
    const hasValue = value !== undefined && value !== null && value !== '';
    const shouldShrink = isFocused || hasValue;

    // Extract onFocus/onBlur from inputProps since we chain them in our handlers
    const { onFocus: _onFocus, onBlur: _onBlur, ...restInputProps } = inputProps || {};

    const startAdornment = InputProps?.startAdornment;

    const rootClasses = clsx(
      styles.root,
      fullWidth && styles.fullWidth,
      margin === 'normal' && styles.marginNormal,
      margin === 'dense' && styles.marginDense,
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
            {label && (
              <label htmlFor={inputId} className={labelClasses}>
                {label}
              </label>
            )}
            <div className={styles.inputContainer}>
              {startAdornment}
              <input
                id={inputId}
                className={styles.standardInput}
                value={value}
                onChange={onChange}
                type={type}
                placeholder={placeholder}
                onFocus={this.handleFocus}
                onBlur={this.handleBlur}
                autoFocus={autoFocus}
                autoComplete={autoComplete}
                name={name}
                onKeyPress={onKeyPress}
                {...restInputProps}
                {...rest}
              />
            </div>
          </div>
          {helperText && <p className={clsx(styles.helperText, error && styles.helperTextError)}>{helperText}</p>}
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
          {label && (
            <label htmlFor={inputId} className={labelClasses}>
              {label}
            </label>
          )}
          <div className={styles.inputContainer}>
            {startAdornment}
            <input
              id={inputId}
              className={styles.outlinedInput}
              value={value}
              onChange={onChange}
              type={type}
              placeholder={placeholder}
              onFocus={this.handleFocus}
              onBlur={this.handleBlur}
              autoFocus={autoFocus}
              autoComplete={autoComplete}
              name={name}
              onKeyPress={onKeyPress}
              {...restInputProps}
              {...rest}
            />
          </div>
        </div>
        {helperText && <p className={clsx(styles.helperText, error && styles.helperTextError)}>{helperText}</p>}
      </div>
    );
  }
}
