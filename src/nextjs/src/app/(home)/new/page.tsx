import { redirect } from 'next/navigation';

import getAuthenticatedServerApp from '@/lib/firebase/serverApp';
import NewProjectForm from './NewProjectForm';

export default async function NewProject() {
  const { currentUser } = await getAuthenticatedServerApp();

  if (!currentUser) redirect('/login');

  return <NewProjectForm userId={currentUser.uid} />;
}
