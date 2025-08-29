import { redirect } from 'next/navigation';
import { getAuthenticatedAppForUser } from './firebase/serverApp';

export default async function getSignedInUser() {
  const { currentUser } = await getAuthenticatedAppForUser();
  if (!currentUser) redirect('/login');
  else return currentUser;
}
