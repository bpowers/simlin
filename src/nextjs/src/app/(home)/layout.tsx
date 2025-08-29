import { redirect } from 'next/navigation';

import { getAuthenticatedAppForUser } from '@/lib/firebase/serverApp';
import Header from './Header';

export default async function Home({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  const { currentUser } = await getAuthenticatedAppForUser();

  if (!currentUser) redirect('/login');

  return (
    <div>
      <Header user={currentUser.toJSON() as { uid: string }} />
      {children}
    </div>
  );
}
