import { redirect } from 'next/navigation';

import getAuthenticatedServerApp from '@/lib/firebase/serverApp';
import Header from './Header';

export default async function Home({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  const { currentUser } = await getAuthenticatedServerApp();

  if (!currentUser) redirect('/login');

  return (
    <div>
      <Header user={currentUser.toJSON() as { uid: string }} />
      {children}
    </div>
  );
}
