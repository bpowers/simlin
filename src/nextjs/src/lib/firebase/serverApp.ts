import 'server-only';

import { initializeServerApp } from 'firebase/app';
import { cookies } from 'next/headers';
import { app, getAuth } from './helpers';

export async function getAuthenticatedAppForUser() {
  const authIdToken = (await cookies()).get('__session')?.value;

  const firebaseServerApp = initializeServerApp(app, { authIdToken });

  const auth = await getAuth(firebaseServerApp);

  return { firebaseServerApp, currentUser: auth.currentUser };
}
