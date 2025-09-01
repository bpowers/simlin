import 'server-only';

import { initializeServerApp } from 'firebase/app';
import { cookies, headers } from 'next/headers';
import { app, getAuth } from './helpers';
import { redirect } from 'next/navigation';

export default async function getAuthenticatedServerApp() {
  const authIdToken = (await cookies()).get('__session')?.value;
  const headersObj = await headers();

  try {
    const firebaseServerApp = initializeServerApp(app, { authIdToken, releaseOnDeref: headersObj });

    const auth = await getAuth(firebaseServerApp);

    return { firebaseServerApp, currentUser: auth.currentUser };
  } catch {
    redirect('/refresh-token');
  }
}
