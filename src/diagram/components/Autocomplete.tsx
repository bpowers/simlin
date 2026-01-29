// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import ReactDOM from 'react-dom';

import { useCombobox } from 'downshift';
import clsx from 'clsx';

import styles from './Autocomplete.module.css';

interface AutocompleteProps {
  key?: string;
  value?: string | null;
  defaultValue?: string;
  onChange: (event: any, newValue: string | null) => void;
  clearOnEscape?: boolean;
  options: string[];
  renderInput: (params: any) => React.ReactNode;
}

function itemToString(item: string | null): string {
  return item || '';
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

  const {
    isOpen,
    getInputProps,
    getMenuProps,
    getItemProps,
    highlightedIndex,
  } = useCombobox({
    items: filteredOptions,
    itemToString,
    inputValue,
    selectedItem: value || null,
    onInputValueChange: ({ inputValue: newInputValue }) => {
      setInputValue(newInputValue || '');
    },
    onSelectedItemChange: ({ selectedItem }) => {
      onChange(null, selectedItem || null);
    },
    stateReducer: (state, actionAndChanges) => {
      const { type, changes } = actionAndChanges;
      if (clearOnEscape && type === useCombobox.stateChangeTypes.InputKeyDownEscape) {
        return {
          ...changes,
          selectedItem: null,
          inputValue: '',
        };
      }
      return changes;
    },
  });

  React.useEffect(() => {
    if (isOpen && wrapperRef.current) {
      const rect = wrapperRef.current.getBoundingClientRect();
      setDropdownPosition({
        top: rect.bottom + window.scrollY,
        left: rect.left + window.scrollX,
        width: rect.width,
      });
    }
  }, [isOpen]);

  const inputProps = getInputProps();
  const params = {
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
