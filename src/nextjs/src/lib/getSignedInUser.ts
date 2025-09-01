import { redirect } from 'next/navigation';

import getAuthenticatedServerApp from './firebase/serverApp';

export default async function getSignedInUser() {
  const { currentUser } = await getAuthenticatedServerApp();
  if (!currentUser) redirect('/login');
  else return currentUser;
}
