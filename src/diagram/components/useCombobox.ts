// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

// A small, dependency-free combobox state machine covering exactly what the
// Autocomplete component needs: a filter-as-you-type listbox over a string[]
// with keyboard navigation and the WAI-ARIA combobox attributes. It replaces
// downshift's useCombobox (whose full feature surface we never used) while
// preserving the same return shape (getInputProps/getMenuProps/getItemProps/
// isOpen/highlightedIndex) so callers compose it identically.

export interface UseComboboxConfig {
  /** The currently displayed (already-filtered) items. */
  items: string[];
  /** The controlled text shown in the input. */
  inputValue: string;
  /** Emitted on every keystroke; the caller owns inputValue. */
  onInputValueChange: (changes: { inputValue: string }) => void;
  /** Emitted when an item is chosen (click or Enter) or cleared via Escape. */
  onSelectedItemChange: (changes: { selectedItem: string | null }) => void;
  /** When true, Escape clears the selection and input (else it just closes). */
  clearOnEscape?: boolean;
}

export interface ComboboxInputProps {
  role: 'combobox';
  'aria-expanded': boolean;
  'aria-controls': string;
  'aria-autocomplete': 'list';
  'aria-activedescendant'?: string;
  autoComplete: 'off';
  id: string;
  value: string;
  onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onKeyDown: (event: React.KeyboardEvent<HTMLInputElement>) => void;
  onClick: () => void;
  onBlur: () => void;
}

export interface ComboboxMenuProps {
  role: 'listbox';
  id: string;
}

export interface ComboboxItemProps {
  role: 'option';
  id: string;
  'aria-selected': boolean;
  onMouseMove: () => void;
  onMouseDown: (event: React.MouseEvent) => void;
  onClick: () => void;
}

export interface UseComboboxResult {
  isOpen: boolean;
  highlightedIndex: number;
  getInputProps: () => ComboboxInputProps;
  getMenuProps: () => ComboboxMenuProps;
  getItemProps: (options: { item: string; index: number }) => ComboboxItemProps;
}

export function useCombobox(config: UseComboboxConfig): UseComboboxResult {
  const { items, inputValue, onInputValueChange, onSelectedItemChange, clearOnEscape } = config;

  const [isOpen, setIsOpen] = React.useState(false);
  const [highlightedIndex, setHighlightedIndex] = React.useState(-1);

  const baseId = React.useId();
  const menuId = `${baseId}-menu`;
  const inputId = `${baseId}-input`;
  const itemId = (index: number) => `${baseId}-item-${index}`;

  // Whether a mouse press is what is causing the current input blur. A blur from
  // a click (dismissing the list or pressing elsewhere) must NOT auto-commit the
  // highlighted option; only a keyboard-driven blur (tab away) accepts it, which
  // matches downshift. The click-induced blur fires synchronously in the same
  // task as the mousedown, so a deferred reset clears the flag right after that
  // blur -- it can never strand true (no reliance on a mouseup that a release
  // outside the window might never deliver).
  const pointerDownRef = React.useRef(false);
  React.useEffect(() => {
    const onPointerDown = (): void => {
      pointerDownRef.current = true;
      setTimeout(() => {
        pointerDownRef.current = false;
      }, 0);
    };
    document.addEventListener('mousedown', onPointerDown, true);
    return () => {
      document.removeEventListener('mousedown', onPointerDown, true);
    };
  }, []);

  // Keep the keyboard-highlighted option visible: focus stays on the input
  // (combobox pattern), so arrowing past the fold of a list taller than the
  // popup would otherwise move the highlight off-screen with nothing scrolling.
  // `block: 'nearest'` only scrolls when the row is actually out of view.
  React.useEffect(() => {
    if (!isOpen || highlightedIndex < 0) {
      return;
    }
    const el = document.getElementById(`${baseId}-item-${highlightedIndex}`);
    el?.scrollIntoView?.({ block: 'nearest' });
  }, [isOpen, highlightedIndex, baseId]);

  const selectItem = React.useCallback(
    (item: string) => {
      // Set the field text to the chosen item (downshift did this). Without it
      // an uncontrolled consumer -- one that does not feed `value` back in --
      // would keep showing the partial text the user typed, not the selection.
      onInputValueChange({ inputValue: item });
      onSelectedItemChange({ selectedItem: item });
      setIsOpen(false);
      setHighlightedIndex(-1);
    },
    [onInputValueChange, onSelectedItemChange],
  );

  const handleKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    switch (event.key) {
      case 'ArrowDown': {
        event.preventDefault();
        if (!isOpen) {
          setIsOpen(true);
          setHighlightedIndex(items.length > 0 ? 0 : -1);
        } else {
          setHighlightedIndex((prev) => Math.min(prev + 1, items.length - 1));
        }
        break;
      }
      case 'ArrowUp': {
        event.preventDefault();
        if (!isOpen) {
          setIsOpen(true);
          setHighlightedIndex(items.length - 1);
        } else {
          setHighlightedIndex((prev) => Math.max(prev - 1, 0));
        }
        break;
      }
      case 'Enter': {
        if (isOpen && highlightedIndex >= 0 && highlightedIndex < items.length) {
          event.preventDefault();
          selectItem(items[highlightedIndex]);
        }
        break;
      }
      case 'Escape': {
        if (clearOnEscape) {
          onSelectedItemChange({ selectedItem: null });
          onInputValueChange({ inputValue: '' });
        }
        setIsOpen(false);
        setHighlightedIndex(-1);
        break;
      }
      default:
        break;
    }
  };

  const getInputProps = (): ComboboxInputProps => ({
    role: 'combobox',
    'aria-expanded': isOpen,
    'aria-controls': menuId,
    'aria-autocomplete': 'list',
    'aria-activedescendant': isOpen && highlightedIndex >= 0 ? itemId(highlightedIndex) : undefined,
    autoComplete: 'off',
    id: inputId,
    value: inputValue,
    onChange: (event) => {
      onInputValueChange({ inputValue: event.target.value });
      setIsOpen(true);
      setHighlightedIndex(-1);
    },
    onKeyDown: handleKeyDown,
    // Clicking the field opens the list (showing all items when empty), the
    // same affordance downshift gave the wiring editor's selects.
    onClick: () => setIsOpen(true),
    onBlur: () => {
      // A keyboard blur (tab away) with an active highlight accepts that option,
      // the way downshift did, so keyboard users don't lose the choice they
      // navigated to. A mouse-driven blur (click elsewhere) just closes.
      if (!pointerDownRef.current && isOpen && highlightedIndex >= 0 && highlightedIndex < items.length) {
        selectItem(items[highlightedIndex]);
      } else {
        setIsOpen(false);
        setHighlightedIndex(-1);
      }
    },
  });

  const getMenuProps = (): ComboboxMenuProps => ({
    role: 'listbox',
    id: menuId,
  });

  const getItemProps = ({ item, index }: { item: string; index: number }): ComboboxItemProps => ({
    role: 'option',
    id: itemId(index),
    'aria-selected': highlightedIndex === index,
    onMouseMove: () => setHighlightedIndex(index),
    // Keep focus on the input so the trailing blur doesn't beat the click and
    // close the listbox before the selection registers.
    onMouseDown: (event) => event.preventDefault(),
    onClick: () => selectItem(item),
  });

  return { isOpen, highlightedIndex, getInputProps, getMenuProps, getItemProps };
}
