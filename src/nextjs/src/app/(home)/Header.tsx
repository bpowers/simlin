'use client';

import { AppBar, Toolbar, IconButton, Typography, Button, Menu, MenuItem, Avatar } from '@mui/material';
import { AccountCircle, Menu as MenuIcon } from '@mui/icons-material';
import Link from 'next/link';
import { useRef, useState } from 'react';

import { signOut } from '@/lib/firebase/auth';
import useUserSession from '@/lib/hooks/useUserSession';

const anchorOrigin = {
  vertical: 'bottom',
  horizontal: 'right',
} as const;

export interface Props {
  user: { uid: string; photoURL?: string; displayName?: string };
  hideNewButton?: boolean;
}

export default function Header({ user, hideNewButton }: Props) {
  const [isMenuOpen, setIsMenuOpen] = useState(false);
  const menuAnchor = useRef<HTMLButtonElement>(null);

  useUserSession(user.uid);

  const { photoURL, displayName } = user;

  return (
    <AppBar position="fixed">
      <Toolbar variant="dense">
        <IconButton sx={{ color: 'inherit', marginRight: '20px' }} aria-label="Menu">
          <MenuIcon />
        </IconButton>
        <Link href="/" className="flex-1">
          <Typography variant="h6" className="text-white">
            Simlin
          </Typography>
        </Link>
        <div>
          {!hideNewButton && (
            <Link href="/new">
              <Button variant="outlined" sx={{ color: 'white', borderColor: 'white', marginRight: '16px' }}>
                New Project
              </Button>
            </Link>
          )}

          <IconButton
            className="p-2 text-inherit"
            aria-owns={isMenuOpen ? 'menu-appbar' : undefined}
            aria-haspopup="true"
            onClick={() => setIsMenuOpen((state) => !state)}
            ref={menuAnchor}
            sx={{ color: 'inherit' }}
          >
            {photoURL ? (
              <Avatar
                alt={displayName ?? 'Your profile picture'}
                src={photoURL}
                sx={{ height: '32px', width: '32px' }}
              />
            ) : (
              <AccountCircle />
            )}
          </IconButton>
          <Menu
            anchorEl={menuAnchor.current}
            anchorOrigin={anchorOrigin}
            transformOrigin={anchorOrigin}
            open={isMenuOpen}
            onClose={() => setIsMenuOpen(false)}
          >
            <MenuItem onClick={signOut}>Logout</MenuItem>
          </Menu>
        </div>
      </Toolbar>
    </AppBar>
  );
}
