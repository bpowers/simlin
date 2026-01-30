// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen, act } from '@testing-library/react';
import SpeedDial, { SpeedDialAction, SpeedDialIcon, CloseReason } from '../components/SpeedDial';

// Controlled wrapper for testing SpeedDial open/close behavior
class ControlledSpeedDial extends React.Component<
  { children?: React.ReactNode },
  { open: boolean; lastCloseReason: CloseReason | null }
> {
  state = { open: false, lastCloseReason: null as CloseReason | null };

  setOpen = (open: boolean) => {
    this.setState({ open });
  };

  handleClick = () => {
    this.setState((prev) => ({ open: !prev.open }));
  };

  handleClose = (_event: React.SyntheticEvent, reason: CloseReason) => {
    this.setState({ open: false, lastCloseReason: reason });
  };

  render() {
    return (
      <SpeedDial
        ariaLabel="Test SpeedDial"
        open={this.state.open}
        onClick={this.handleClick}
        onClose={this.handleClose}
        icon={<SpeedDialIcon icon={<span>+</span>} openIcon={<span>x</span>} />}
      >
        {this.props.children}
      </SpeedDial>
    );
  }
}

describe('SpeedDial', () => {
  test('renders FAB button with aria-label', () => {
    render(<SpeedDial ariaLabel="Test SpeedDial" open={false} icon={<span>+</span>} />);

    const fab = screen.getByRole('button', { name: 'Test SpeedDial' });
    expect(fab).not.toBeNull();
  });

  test('renders when hidden prop is true (visibility controlled by CSS)', () => {
    const { container } = render(
      <SpeedDial ariaLabel="Test SpeedDial" open={false} hidden={true} icon={<span>+</span>} />,
    );

    // Element is still rendered (visibility controlled by CSS)
    const speedDial = container.firstChild as HTMLElement;
    expect(speedDial).not.toBeNull();
    // FAB button is still present
    const fab = screen.getByRole('button', { name: 'Test SpeedDial' });
    expect(fab).not.toBeNull();
  });

  test('renders actions when open', () => {
    render(
      <SpeedDial ariaLabel="Test SpeedDial" open={true} icon={<span>+</span>}>
        <SpeedDialAction icon={<span>A</span>} title="Action A" />
        <SpeedDialAction icon={<span>B</span>} title="Action B" />
      </SpeedDial>,
    );

    const actions = screen.getAllByRole('menuitem');
    expect(actions.length).toBe(2);
  });

  test('does not render actions when closed', () => {
    render(
      <SpeedDial ariaLabel="Test SpeedDial" open={false} icon={<span>+</span>}>
        <SpeedDialAction icon={<span>A</span>} title="Action A" />
      </SpeedDial>,
    );

    const actions = screen.queryAllByRole('menuitem');
    expect(actions.length).toBe(0);
  });

  test('calls onClick when FAB is clicked', () => {
    const onClick = jest.fn();
    render(<SpeedDial ariaLabel="Test SpeedDial" open={false} onClick={onClick} icon={<span>+</span>} />);

    const fab = screen.getByRole('button', { name: 'Test SpeedDial' });
    fireEvent.click(fab);

    expect(onClick).toHaveBeenCalledTimes(1);
  });

  test('calls onClose with mouseLeave reason on mouse leave', () => {
    const ref = React.createRef<ControlledSpeedDial>();
    const { container } = render(<ControlledSpeedDial ref={ref} />);

    act(() => {
      ref.current!.setOpen(true);
    });

    const speedDial = container.firstChild as HTMLElement;
    fireEvent.mouseLeave(speedDial);

    expect(ref.current!.state.lastCloseReason).toBe('mouseLeave');
  });

  test('calls onClose with blur reason on FAB blur', () => {
    const ref = React.createRef<ControlledSpeedDial>();
    render(<ControlledSpeedDial ref={ref} />);

    act(() => {
      ref.current!.setOpen(true);
    });

    const fab = screen.getByRole('button', { name: 'Test SpeedDial' });
    fireEvent.blur(fab);

    expect(ref.current!.state.lastCloseReason).toBe('blur');
  });

  test('calls onClose with escapeKeyDown reason on Escape key', () => {
    const ref = React.createRef<ControlledSpeedDial>();
    const { container } = render(<ControlledSpeedDial ref={ref} />);

    act(() => {
      ref.current!.setOpen(true);
    });

    const speedDial = container.firstChild as HTMLElement;
    fireEvent.keyDown(speedDial, { key: 'Escape' });

    expect(ref.current!.state.lastCloseReason).toBe('escapeKeyDown');
  });

  test('sets aria-expanded based on open state', () => {
    const { rerender } = render(<SpeedDial ariaLabel="Test SpeedDial" open={false} icon={<span>+</span>} />);

    let fab = screen.getByRole('button', { name: 'Test SpeedDial' });
    expect(fab.getAttribute('aria-expanded')).toBe('false');

    rerender(<SpeedDial ariaLabel="Test SpeedDial" open={true} icon={<span>+</span>} />);

    fab = screen.getByRole('button', { name: 'Test SpeedDial' });
    expect(fab.getAttribute('aria-expanded')).toBe('true');
  });

  test('applies custom className', () => {
    const { container } = render(
      <SpeedDial ariaLabel="Test SpeedDial" open={false} icon={<span>+</span>} className="custom-class" />,
    );

    const speedDial = container.firstChild as HTMLElement;
    expect(speedDial.className).toContain('custom-class');
  });
});

describe('SpeedDialAction', () => {
  test('renders button with icon', () => {
    render(
      <SpeedDial ariaLabel="Test" open={true} icon={<span>+</span>}>
        <SpeedDialAction icon={<span data-testid="action-icon">A</span>} title="Action A" />
      </SpeedDial>,
    );

    const icon = screen.getByTestId('action-icon');
    expect(icon).not.toBeNull();
  });

  test('renders action button with aria-label for accessibility', () => {
    render(
      <SpeedDial ariaLabel="Test" open={true} icon={<span>+</span>}>
        <SpeedDialAction icon={<span>A</span>} title="Action A" />
      </SpeedDial>,
    );

    // The title appears as aria-label on button (tooltips appear on hover/focus)
    const button = screen.getByRole('button', { name: 'Action A' });
    expect(button).not.toBeNull();
    expect(button.getAttribute('aria-label')).toBe('Action A');
  });

  test('calls onClick when action button is clicked', () => {
    const onClick = jest.fn();
    render(
      <SpeedDial ariaLabel="Test" open={true} icon={<span>+</span>}>
        <SpeedDialAction icon={<span>A</span>} title="Action A" onClick={onClick} />
      </SpeedDial>,
    );

    const button = screen.getByRole('button', { name: 'Action A' });
    fireEvent.click(button);

    expect(onClick).toHaveBeenCalledTimes(1);
  });

  test('action click does not trigger onClose', () => {
    const onClose = jest.fn();
    render(
      <SpeedDial ariaLabel="Test" open={true} onClose={onClose} icon={<span>+</span>}>
        <SpeedDialAction icon={<span>A</span>} title="Action A" />
      </SpeedDial>,
    );

    const button = screen.getByRole('button', { name: 'Action A' });
    fireEvent.click(button);

    expect(onClose).not.toHaveBeenCalled();
  });

  test('selected action has selected class', () => {
    render(
      <SpeedDial ariaLabel="Test" open={true} icon={<span>+</span>}>
        <SpeedDialAction icon={<span>A</span>} title="Action A" selected={true} />
      </SpeedDial>,
    );

    const button = screen.getByRole('button', { name: 'Action A' });
    expect(button.className).toContain('actionButtonSelected');
  });

  test('applies custom className to action button', () => {
    render(
      <SpeedDial ariaLabel="Test" open={true} icon={<span>+</span>}>
        <SpeedDialAction icon={<span>A</span>} title="Action A" className="custom-action-class" />
      </SpeedDial>,
    );

    const button = screen.getByRole('button', { name: 'Action A' });
    expect(button.className).toContain('custom-action-class');
  });
});

describe('SpeedDialIcon', () => {
  test('renders icon when closed', () => {
    render(
      <SpeedDial
        ariaLabel="Test"
        open={false}
        icon={
          <SpeedDialIcon
            icon={<span data-testid="closed-icon">+</span>}
            openIcon={<span data-testid="open-icon">x</span>}
          />
        }
      />,
    );

    expect(screen.getByTestId('closed-icon')).not.toBeNull();
    expect(screen.queryByTestId('open-icon')).toBeNull();
  });

  test('renders openIcon when open', () => {
    render(
      <SpeedDial
        ariaLabel="Test"
        open={true}
        icon={
          <SpeedDialIcon
            icon={<span data-testid="closed-icon">+</span>}
            openIcon={<span data-testid="open-icon">x</span>}
          />
        }
      />,
    );

    expect(screen.queryByTestId('closed-icon')).toBeNull();
    expect(screen.getByTestId('open-icon')).not.toBeNull();
  });

  test('renders icon wrapper consistently when no openIcon provided', () => {
    const { rerender } = render(
      <SpeedDial ariaLabel="Test" open={false} icon={<SpeedDialIcon icon={<span data-testid="the-icon">+</span>} />} />,
    );

    // Icon is present when closed
    let icon = screen.getByTestId('the-icon');
    expect(icon).not.toBeNull();

    rerender(
      <SpeedDial ariaLabel="Test" open={true} icon={<SpeedDialIcon icon={<span data-testid="the-icon">+</span>} />} />,
    );

    // Icon is still present when open (rotation is CSS-based)
    icon = screen.getByTestId('the-icon');
    expect(icon).not.toBeNull();
  });

  test('passes open prop to icon via cloneElement with type safety', () => {
    // This tests that the type-safe cloneElement works correctly
    const CustomIcon = ({ open }: { open?: boolean }) => (
      <span data-testid="custom-icon" data-open={String(open)}>
        {open ? 'Open' : 'Closed'}
      </span>
    );

    render(<SpeedDial ariaLabel="Test" open={true} icon={<CustomIcon />} />);

    const icon = screen.getByTestId('custom-icon');
    expect(icon.getAttribute('data-open')).toBe('true');
  });
});
