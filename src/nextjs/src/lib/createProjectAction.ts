'use server';

import { redirect } from 'next/navigation';
import { createProject } from './database/projects';
import getAuthenticatedServerApp from './firebase/serverApp';

const DEFAULT_ISPUBLIC = false;

function validatedFormData(formData: FormData) {
  const projectName = formData.get('name');
  if (typeof projectName !== 'string' || projectName.length === 0) throw new Error('Invalid name format');

  const description = formData.get('description');
  if (description instanceof File) throw new Error('Invalid description format');

  const modelFile = formData.get('model-file');
  if (modelFile && typeof modelFile !== 'string') throw new Error('Model must be passed encoded as string');

  const isPublic = !!formData.get('is-public');

  return {
    projectName,
    description,
    modelFile,
    isPublic,
  };
}

export default async function createProjectAction(
  _: { errorMessage?: string; formData: FormData },
  projectData: FormData,
): Promise<{ errorMessage?: string; formData: FormData }> {
  const { currentUser } = await getAuthenticatedServerApp();
  if (!currentUser) redirect('/login');
  const ownerId = currentUser.uid;

  const { projectName, isPublic, modelFile, description } = validatedFormData(projectData);

  const initialProjectBinary = modelFile ? new TextEncoder().encode(modelFile) : undefined;

  const newProject = {
    ownerId,
    isPublic,
    displayName: projectName,
    ...(initialProjectBinary ? { initialProjectBinary } : {}),
    ...(description ? { description } : {}),
  };

  try {
    await createProject(newProject);
  } catch (e) {
    return { errorMessage: e instanceof Error ? e.message : 'Unknown error', formData: projectData };
  }
  redirect(`/${newProject.ownerId}/${projectName}`);
}
