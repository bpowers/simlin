import { useEffect } from 'react';
import { onIdTokenChanged } from '../firebase/auth';
import { serialize } from 'cookie';

export default function useOnIdTokenChange(initialUser?: { uid: string }) {
  useEffect(() => {
    return onIdTokenChanged(async (user) => {
      if (user) {
        const idToken = await user.getIdToken();
        const cookieStr = serialize('__session', idToken, { path: '/' });
        document.cookie = cookieStr;
      } else {
        const cookieStr = serialize('__session', '', { maxAge: -1 });
        document.cookie = cookieStr;
      }
      if (initialUser?.uid === user?.uid) {
        return;
      }
      window.location.reload();
    });
  }, [initialUser]);
}
