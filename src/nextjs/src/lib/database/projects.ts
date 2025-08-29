import { doc, getDoc, collection, query, where, getDocs, deleteDoc, addDoc } from 'firebase/firestore';
import db from './db';
import { Project } from './models';

const COLLECTION_ID = 'projects';

const projects = collection(db, COLLECTION_ID);

export async function getProject(id: string) {
  const ref = doc(db, COLLECTION_ID, id);
  const project = await getDoc(ref);

  return project.exists() ? (project.data() as Project) : undefined;
}

export async function getProjects(username: string) {
  const q = query(projects, where('ownerId', '==', username));
  const ref = await getDocs(q);

  return ref.docs.map((p) => ({ id: p.id, ...p.data() }) as Project);
}

export async function createProject(project: Omit<Project, 'id'>) {
  return addDoc(projects, project);
}
