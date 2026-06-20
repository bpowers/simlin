// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import ReactDOM from 'react-dom';

import clsx from 'clsx';

import styles from './Autocomplete.module.css';
import { useCombobox } from './useCombobox';

export interface AutocompleteRenderInputParams {
  InputProps: {
    disableUnderline: boolean;
    ref: React.Ref<HTMLDivElement>;
  };
  inputProps: React.InputHTMLAttributes<HTMLInputElement>;
}

interface AutocompleteProps {
  key?: string;
  value?: string | null;
  defaultValue?: string;
  onChange: (event: React.SyntheticEvent | null, newValue: string | null) => void;
  clearOnEscape?: boolean;
  options: string[];
  renderInput: (params: AutocompleteRenderInputParams) => React.ReactNode;
}

export default function Autocomplete(props: AutocompleteProps) {
  const { value, onChange, clearOnEscape, options, renderInput } = props;

  const [inputValue, setInputValue] = React.useState(value || '');
  const wrapperRef = React.useRef<HTMLDivElement>(null);
  const [dropdownPosition, setDropdownPosition] = React.useState<{
    top: number;
    left: number;
    width: number;
  } | null>(null);

  // Sync inputValue when value prop changes externally
  React.useEffect(() => {
    setInputValue(value || '');
  }, [value]);

  const filteredOptions = React.useMemo(() => {
    if (!inputValue) return options;
    const lower = inputValue.toLowerCase();
    return options.filter((opt) => opt.toLowerCase().includes(lower));
  }, [options, inputValue]);

  const { isOpen, getInputProps, getMenuProps, getItemProps, highlightedIndex } = useCombobox({
    items: filteredOptions,
    inputValue,
    onInputValueChange: ({ inputValue: newInputValue }) => {
      setInputValue(newInputValue || '');
    },
    onSelectedItemChange: ({ selectedItem }) => {
      onChange(null, selectedItem || null);
    },
    clearOnEscape,
  });

  const updateDropdownPosition = React.useCallback(() => {
    if (wrapperRef.current) {
      const rect = wrapperRef.current.getBoundingClientRect();
      setDropdownPosition({
        top: rect.bottom + window.scrollY,
        left: rect.left + window.scrollX,
        width: rect.width,
      });
    }
  }, []);

  React.useEffect(() => {
    if (!isOpen) {
      return;
    }
    updateDropdownPosition();
    // The listbox is portaled to document.body, so it doesn't move with the
    // input. Recompute while open: capture-phase scroll catches scrolls of
    // any ancestor (e.g. the scrollable details panel hosting the wiring
    // table), not just the window.
    window.addEventListener('scroll', updateDropdownPosition, true);
    window.addEventListener('resize', updateDropdownPosition);
    return () => {
      window.removeEventListener('scroll', updateDropdownPosition, true);
      window.removeEventListener('resize', updateDropdownPosition);
    };
  }, [isOpen, updateDropdownPosition]);

  const inputProps = getInputProps() as React.InputHTMLAttributes<HTMLInputElement>;
  const params: AutocompleteRenderInputParams = {
    InputProps: {
      disableUnderline: false,
      ref: wrapperRef,
    },
    inputProps,
  };

  const menuProps = getMenuProps();

  const listbox =
    isOpen && filteredOptions.length > 0 && dropdownPosition ? (
      <ul
        {...menuProps}
        className={styles.listbox}
        style={{
          position: 'absolute',
          top: dropdownPosition.top,
          left: dropdownPosition.left,
          width: dropdownPosition.width,
        }}
      >
        {filteredOptions.map((item, index) => (
          <li
            key={item}
            {...getItemProps({ item, index })}
            className={clsx(styles.option, highlightedIndex === index && styles.optionHighlighted)}
          >
            {item}
          </li>
        ))}
      </ul>
    ) : (
      <ul {...menuProps} style={{ display: 'none' }} />
    );

  return (
    <div className={styles.wrapper}>
      {renderInput(params)}
      {ReactDOM.createPortal(listbox, document.body)}
    </div>
  );
}
