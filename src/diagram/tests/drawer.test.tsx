// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, waitFor, act } from '@testing-library/react';
import Drawer from '../components/Drawer';

// Controlled wrapper for Drawer to test open/close behavior. Exposes the same
// imperative surface the old class component did -- `setOpen` plus a live
// `state` object -- so tests can drive and inspect it through a ref.
interface ControlledDrawerHandle {
  setOpen: (open: boolean) => void;
  state: { open: boolean; closeCount: number };
}

const ControlledDrawer = React.forwardRef<ControlledDrawerHandle, { children?: React.ReactNode }>(
  function ControlledDrawer({ children }, ref) {
    const [open, setOpen] = React.useState(false);
    const [closeCount, setCloseCount] = React.useState(0);

    const handleClose = () => {
      setOpen(false);
      setCloseCount((prev) => prev + 1);
    };

    React.useImperativeHandle(ref, () => ({ setOpen, state: { open, closeCount } }), [open, closeCount]);

    return (
      <Drawer open={open} onClose={handleClose}>
        {children}
      </Drawer>
    );
  },
);

describe('Drawer', () => {
  test('renders children when open', () => {
    render(
      <Drawer open={true} onClose={() => {}}>
        <div data-testid="drawer-content">Content</div>
      </Drawer>,
    );

    const content = document.querySelector('[data-testid="drawer-content"]');
    expect(content).not.toBeNull();
  });

  test('renders panel even when closed (for CSS transitions)', () => {
    render(
      <Drawer open={false} onClose={() => {}}>
        <div data-testid="drawer-content">Content</div>
      </Drawer>,
    );

    // Panel is always rendered (visibility controlled by CSS)
    const panel = document.querySelector('[role="dialog"]');
    expect(panel).not.toBeNull();
    // Content is present
    const content = document.querySelector('[data-testid="drawer-content"]');
    expect(content).not.toBeNull();
  });

  test('renders backdrop even when closed (for CSS transitions)', () => {
    render(
      <Drawer open={false} onClose={() => {}}>
        <div>Content</div>
      </Drawer>,
    );

    // Backdrop is always rendered (visibility controlled by CSS)
    const backdrop = document.querySelector('[aria-hidden="true"]');
    expect(backdrop).not.toBeNull();
  });

  test('calls onClose when backdrop is clicked', () => {
    const ref = React.createRef<ControlledDrawerHandle>();
    render(
      <ControlledDrawer ref={ref}>
        <div>Content</div>
      </ControlledDrawer>,
    );

    // Open the drawer first
    act(() => {
      ref.current!.setOpen(true);
    });

    const backdrop = document.querySelector('[aria-hidden="true"]');
    fireEvent.click(backdrop!);

    expect(ref.current!.state.closeCount).toBe(1);
    expect(ref.current!.state.open).toBe(false);
  });

  test('calls onClose when Escape key is pressed', () => {
    const ref = React.createRef<ControlledDrawerHandle>();
    render(
      <ControlledDrawer ref={ref}>
        <div>Content</div>
      </ControlledDrawer>,
    );

    // Open the drawer first
    act(() => {
      ref.current!.setOpen(true);
    });

    fireEvent.keyDown(document, { key: 'Escape' });

    expect(ref.current!.state.closeCount).toBe(1);
    expect(ref.current!.state.open).toBe(false);
  });

  test('does not call onClose when Escape key is pressed while closed', () => {
    const ref = React.createRef<ControlledDrawerHandle>();
    render(
      <ControlledDrawer ref={ref}>
        <div>Content</div>
      </ControlledDrawer>,
    );

    fireEvent.keyDown(document, { key: 'Escape' });

    expect(ref.current!.state.closeCount).toBe(0);
  });

  test('focuses the panel when opened', async () => {
    const ref = React.createRef<ControlledDrawerHandle>();
    render(
      <ControlledDrawer ref={ref}>
        <div>Content</div>
      </ControlledDrawer>,
    );

    act(() => {
      ref.current!.setOpen(true);
    });

    await waitFor(() => {
      const panel = document.querySelector('[role="dialog"]');
      expect(document.activeElement).toBe(panel);
    });
  });

  test('restores focus to previous element when closed', async () => {
    // Create a button that will have focus before the drawer opens
    const buttonRef = React.createRef<HTMLButtonElement>();
    const ref = React.createRef<ControlledDrawerHandle>();

    render(
      <>
        <button ref={buttonRef}>Outside Button</button>
        <ControlledDrawer ref={ref}>
          <div>Content</div>
        </ControlledDrawer>
      </>,
    );

    // Focus the button
    buttonRef.current!.focus();
    expect(document.activeElement).toBe(buttonRef.current);

    // Open the drawer
    act(() => {
      ref.current!.setOpen(true);
    });

    await waitFor(() => {
      const panel = document.querySelector('[role="dialog"]');
      expect(document.activeElement).toBe(panel);
    });

    // Close the drawer
    act(() => {
      ref.current!.setOpen(false);
    });

    await waitFor(() => {
      expect(document.activeElement).toBe(buttonRef.current);
    });
  });

  test('restores focus to the pre-open element when mounted open under StrictMode', async () => {
    // Regression guard: StrictMode double-invokes the mount effect, so the focus
    // effect's body runs twice for a Drawer that mounts with open===true. The
    // first run saves the real prior focus and focuses the panel; without the
    // `activeElement === panel` guard the second run would overwrite
    // previousActiveElement with the panel itself, so a later close would
    // "restore" focus to the hidden drawer instead of the button focused before
    // the drawer mounted.
    const buttonRef = React.createRef<HTMLButtonElement>();
    const openRef = React.createRef<{ close: () => void }>();

    // Wrapper that mounts the Drawer ALREADY OPEN (the case the double-invoked
    // mount effect exercises) and exposes a way to close it.
    function MountOpenDrawer(): React.ReactElement {
      const [open, setOpen] = React.useState(true);
      React.useImperativeHandle(openRef, () => ({ close: () => setOpen(false) }));
      return (
        <Drawer open={open} onClose={() => setOpen(false)}>
          <button>Inside Button</button>
        </Drawer>
      );
    }

    // Focus the outside button before the StrictMode subtree (with the open
    // Drawer) mounts.
    render(<button ref={buttonRef}>Outside Button</button>);
    buttonRef.current!.focus();
    expect(document.activeElement).toBe(buttonRef.current);

    render(
      <React.StrictMode>
        <MountOpenDrawer />
      </React.StrictMode>,
    );

    await waitFor(() => {
      const panel = document.querySelector('[role="dialog"]');
      expect(document.activeElement).toBe(panel);
    });

    act(() => {
      openRef.current!.close();
    });

    await waitFor(() => {
      expect(document.activeElement).toBe(buttonRef.current);
    });
  });
});

describe('Drawer focus trap', () => {
  test('traps focus within drawer when Tab is pressed', () => {
    render(
      <Drawer open={true} onClose={() => {}}>
        <button data-testid="first-btn">First</button>
        <button data-testid="second-btn">Second</button>
      </Drawer>,
    );

    const firstBtn = document.querySelector('[data-testid="first-btn"]') as HTMLElement;
    const secondBtn = document.querySelector('[data-testid="second-btn"]') as HTMLElement;

    // Focus the last button
    secondBtn.focus();
    expect(document.activeElement).toBe(secondBtn);

    // Tab should wrap to first button
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: false });
    expect(document.activeElement).toBe(firstBtn);
  });

  test('traps focus when Shift+Tab is pressed', () => {
    render(
      <Drawer open={true} onClose={() => {}}>
        <button data-testid="first-btn">First</button>
        <button data-testid="second-btn">Second</button>
      </Drawer>,
    );

    const firstBtn = document.querySelector('[data-testid="first-btn"]') as HTMLElement;
    const secondBtn = document.querySelector('[data-testid="second-btn"]') as HTMLElement;

    // Focus the first button
    firstBtn.focus();
    expect(document.activeElement).toBe(firstBtn);

    // Shift+Tab should wrap to last button
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(secondBtn);
  });

  test('focus trap includes button elements', () => {
    render(
      <Drawer open={true} onClose={() => {}}>
        <button data-testid="btn">Button</button>
      </Drawer>,
    );

    const panel = document.querySelector('[role="dialog"]');
    const focusable = panel!.querySelectorAll(
      'a, button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"]), [contenteditable]',
    );
    expect(focusable.length).toBe(1);
  });

  test('focus trap includes anchor elements', () => {
    render(
      <Drawer open={true} onClose={() => {}}>
        <a href="#test" data-testid="link">
          Link
        </a>
        <a data-testid="anchor-no-href">Anchor without href</a>
      </Drawer>,
    );

    const panel = document.querySelector('[role="dialog"]');
    const focusable = panel!.querySelectorAll(
      'a, button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"]), [contenteditable]',
    );
    // Both 'a' tags are matched by the 'a' selector
    expect(focusable.length).toBe(2);
  });

  test('focus trap includes input elements', () => {
    render(
      <Drawer open={true} onClose={() => {}}>
        <input type="text" data-testid="input" />
        <select data-testid="select">
          <option>Option</option>
        </select>
        <textarea data-testid="textarea" />
      </Drawer>,
    );

    const panel = document.querySelector('[role="dialog"]');
    const focusable = panel!.querySelectorAll(
      'a, button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"]), [contenteditable]',
    );
    expect(focusable.length).toBe(3);
  });

  test('focus trap includes elements with contenteditable', () => {
    render(
      <Drawer open={true} onClose={() => {}}>
        <div contentEditable suppressContentEditableWarning data-testid="editable">
          Editable
        </div>
      </Drawer>,
    );

    const panel = document.querySelector('[role="dialog"]');
    const focusable = panel!.querySelectorAll(
      'a, button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"]), [contenteditable]',
    );
    expect(focusable.length).toBe(1);
  });

  test('focus trap includes elements with tabindex', () => {
    render(
      <Drawer open={true} onClose={() => {}}>
        <div tabIndex={0} data-testid="focusable-div">
          Focusable div
        </div>
        <div tabIndex={-1} data-testid="not-focusable-div">
          Not focusable via Tab
        </div>
      </Drawer>,
    );

    const panel = document.querySelector('[role="dialog"]');
    const focusable = panel!.querySelectorAll(
      'a, button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"]), [contenteditable]',
    );
    // Only tabIndex=0 should be included, not tabIndex=-1
    expect(focusable.length).toBe(1);
  });

  test('focus trap prevents escape when no focusable elements', () => {
    render(
      <Drawer open={true} onClose={() => {}}>
        <div>No focusable elements here</div>
      </Drawer>,
    );

    const panel = document.querySelector('[role="dialog"]') as HTMLElement;
    panel.focus();

    // Tab should not move focus outside
    const preventDefaultSpy = jest.fn();
    const event = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true });
    Object.defineProperty(event, 'preventDefault', { value: preventDefaultSpy });

    document.dispatchEvent(event);

    expect(preventDefaultSpy).toHaveBeenCalled();
  });
});
