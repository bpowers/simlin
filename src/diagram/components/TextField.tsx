// Copyright 2026 The Simlin Authors. All rights reserved.
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

export default function TextField(props: TextFieldProps): React.ReactElement {
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
  } = props;

  const [isFocused, setIsFocused] = React.useState(false);
  // Generate a stable fallback id exactly once per mount (lazy initializer),
  // mirroring the old constructor's one-shot counter bump.
  const [generatedId] = React.useState(() => `textfield-${++textFieldIdCounter}`);

  const handleFocus = (event: React.FocusEvent<HTMLInputElement>): void => {
    setIsFocused(true);
    // Chain with any external handler from inputProps
    inputProps?.onFocus?.(event);
  };

  const handleBlur = (event: React.FocusEvent<HTMLInputElement>): void => {
    setIsFocused(false);
    // Chain with any external handler from inputProps
    inputProps?.onBlur?.(event);
  };

  const inputId = id || generatedId;

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
      ? clsx(styles.standardLabel, isFocused && styles.standardLabelFocused, error && styles.standardLabelError)
      : undefined;

    return (
      <div className={rootClasses}>
        {label && (
          <label htmlFor={inputId} className={labelClasses}>
            {label}
          </label>
        )}
        <div className={wrapperClasses} ref={wrapperRef}>
          <div className={styles.inputContainer}>
            {startAdornment}
            <input
              className={styles.standardInput}
              value={value}
              onChange={onChange}
              type={type}
              placeholder={placeholder}
              onFocus={handleFocus}
              onBlur={handleBlur}
              autoFocus={autoFocus}
              autoComplete={autoComplete}
              name={name}
              onKeyPress={onKeyPress}
              {...rest}
              {...restInputProps}
              // After the spreads: restInputProps (downshift's
              // getInputProps() when used inside Autocomplete) must win for
              // value/onChange/keyboard handling, but the rendered id has
              // to stay inputId so <label htmlFor={inputId}> keeps pointing
              // at this input.
              id={inputId}
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
    ? clsx(styles.outlinedLabel, isFocused && styles.outlinedLabelFocused, error && styles.outlinedLabelError)
    : undefined;

  return (
    <div className={rootClasses}>
      {label && (
        <label htmlFor={inputId} className={labelClasses}>
          {label}
        </label>
      )}
      <div className={wrapperClasses}>
        <div className={styles.inputContainer}>
          {startAdornment}
          <input
            className={styles.outlinedInput}
            value={value}
            onChange={onChange}
            type={type}
            placeholder={placeholder}
            onFocus={handleFocus}
            onBlur={handleBlur}
            autoFocus={autoFocus}
            autoComplete={autoComplete}
            name={name}
            onKeyPress={onKeyPress}
            {...rest}
            {...restInputProps}
            // See the standard variant: id must survive the spreads so the
            // label association holds.
            id={inputId}
          />
        </div>
      </div>
      {helperText && <p className={clsx(styles.helperText, error && styles.helperTextError)}>{helperText}</p>}
    </div>
  );
}
