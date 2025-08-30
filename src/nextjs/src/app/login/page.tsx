import { redirect } from 'next/navigation';
import ModelIcon from '@system-dynamics/diagram/ModelIcon';
import LoginUI from './LoginUI';
import { getAuthenticatedAppForUser } from '@/lib/firebase/serverApp';

export default async function Login() {
  const { currentUser } = await getAuthenticatedAppForUser();

  if (currentUser) redirect('/');

  return (
    <div className="flex flex-col items-center h-screen justify-center gap-3">
      <ModelIcon className="size-40" />
      <LoginUI />
    </div>
  );
}
