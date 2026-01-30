import * as React from 'react';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import clsx from 'clsx';

import styles from './Menu.module.css';

export interface MenuProps {
  anchorEl: HTMLElement | null;
  open: boolean;
  onClose: () => void;
  anchorOrigin?: { vertical: 'top' | 'bottom'; horizontal: 'left' | 'right' };
  transformOrigin?: { vertical: 'top' | 'bottom'; horizontal: 'left' | 'right' };
  id?: string;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function Menu(props: MenuProps): React.ReactElement {
  const { anchorEl, open, onClose, anchorOrigin, id, className, style, children } = props;

  const side = anchorOrigin?.vertical === 'top' ? 'top' : 'bottom';
  const align = anchorOrigin?.horizontal === 'right' ? 'end' : 'start';

  const anchorRect = React.useMemo(() => anchorEl?.getBoundingClientRect(), [anchorEl]);

  return (
    <DropdownMenu.Root open={open} onOpenChange={(isOpen) => !isOpen && onClose()}>
      <DropdownMenu.Trigger asChild>
        <span
          style={{
            position: 'fixed',
            top: anchorRect?.bottom ?? 0,
            left: anchorRect?.left ?? 0,
            width: anchorRect?.width ?? 0,
            height: 0,
            pointerEvents: 'none',
          }}
        />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          id={id}
          className={clsx(styles.menuContent, className)}
          style={style}
          side={side}
          align={align}
          sideOffset={0}
        >
          {children}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export interface MenuItemProps {
  onClick?: (event: React.MouseEvent) => void;
  disabled?: boolean;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function MenuItem(props: MenuItemProps): React.ReactElement {
  const { onClick, disabled, className, style, children } = props;

  return (
    <DropdownMenu.Item
      className={clsx(styles.menuItem, className)}
      style={style}
      disabled={disabled}
      onSelect={(event) => {
        if (onClick) {
          onClick(event as unknown as React.MouseEvent);
        }
      }}
    >
      {children}
    </DropdownMenu.Item>
  );
}
