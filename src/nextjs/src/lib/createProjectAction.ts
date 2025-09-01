'use server';

import { redirect } from 'next/navigation';
import { createProject } from './database/projects';
import { convertMdlToXmile } from '@system-dynamics/xmutil-js';
import { Project } from '@system-dynamics/core/datamodel';
import { fromXmile } from '@system-dynamics/importer';
import getAuthenticatedServerApp from './firebase/serverApp';
import convertModelFileToBinary from './convertModelFileToBinary';

const DEFAULT_ISPUBLIC = false;

function validatedFormData(formData: FormData) {
  const projectName = formData.get('name');
  if (typeof projectName !== 'string' || projectName.length === 0) throw new Error('Invalid name format');

  const description = formData.get('description');
  if (description instanceof File) throw new Error('Invalid description format');

  const modelFile = formData.get('model-file');
  if (typeof modelFile === 'string') throw new Error('Model file not supported');

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

  let initialProjectBinary: Uint8Array | undefined = undefined;

  if (modelFile) {
    if (modelFile.size !== 0) {
      initialProjectBinary = await convertModelFileToBinary(modelFile);
    }
  }

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
