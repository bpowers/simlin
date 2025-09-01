import { useEffect, useState } from 'react';
import { serialize } from 'cookie';
import { User } from 'firebase/auth';

import { onIdTokenChanged } from '../firebase/auth';

export default function useUserSession(initialUserId?: string) {
  const [currentUser, setCurrentUser] = useState<User>();

  useEffect(() => {
    return onIdTokenChanged(async (user) => {
      try {
        if (user) {
          const idToken = await user.getIdToken();
          const cookieStr = serialize('__session', idToken, { path: '/' });
          document.cookie = cookieStr;
          setCurrentUser(user);
        } else {
          const cookieStr = serialize('__session', '', { maxAge: -1 });
          document.cookie = cookieStr;
          setCurrentUser(undefined);
        }
      } catch (err) {
        if (err instanceof Error) console.error('Firebase connection issue');
        return;
      }
      if (initialUserId === user?.uid) {
        return;
      }
      window.location.reload();
    });
  }, [initialUserId]);

  return currentUser;
}
