import { redirect } from 'next/navigation';

import { getAuthenticatedAppForUser } from '@/lib/firebase/serverApp';

export default async function NewProject() {
  const { currentUser } = await getAuthenticatedAppForUser();

  if (!currentUser) redirect('/login');

  return (
    <div>
      <br />
      <br />
      <br />
      <br />
      <br />
      <br />
      <br />
      <br />
      TEST FORM FOR NEW PROJECT
    </div>
  );
}
