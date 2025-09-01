'use client';

import { useEffect } from 'react';
import { redirect } from 'next/navigation';
import { serialize } from 'cookie';

import { onIdTokenChanged } from '@/lib/firebase/auth';

// This page is needed because Firebase was created for client side applications
// and its architecture is not the best for full-stack frameworks.
// The other options would be to create a service worker to intercept request,
// but it seems more complicated for little benefit.
// See: https://firebase.google.com/docs/auth/web/service-worker-sessions
// See: https://www.reddit.com/r/Firebase/comments/1iatzet/comment/m9i4hoo/
export default function RefreshToken() {
  useEffect(() => {
    return onIdTokenChanged(async (user) => {
      try {
        if (user) {
          const idToken = await user.getIdToken();
          const cookieStr = serialize('__session', idToken, { path: '/' });
          document.cookie = cookieStr;
          redirect('/');
        } else {
          const cookieStr = serialize('__session', '', { maxAge: -1 });
          document.cookie = cookieStr;
          redirect('/login');
        }
      } catch (err) {
        if (err instanceof Error) console.error('Firebase connection issue');
        return;
      }
    });
  }, []);

  return null;
}
