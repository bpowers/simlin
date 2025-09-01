import { redirect } from 'next/navigation';
import ModelIcon from '@system-dynamics/diagram/ModelIcon';

import getAuthenticatedServerApp from '@/lib/firebase/serverApp';
import LoginUI from './LoginUI';

export default async function Login() {
  const { currentUser } = await getAuthenticatedServerApp();

  if (currentUser) redirect('/');

  return (
    <div className="flex flex-col items-center h-screen justify-center gap-3">
      <ModelIcon className="size-40" />
      <LoginUI />
    </div>
  );
}
