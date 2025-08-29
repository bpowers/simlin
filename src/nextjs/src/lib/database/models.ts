export interface File {
  id: string;
  prevIdList: string[];
  projectId: string;
  userId: string;
  created?: number;
  jsonContents: string;
  projectContents: Uint8Array | string;
}

export interface Project {
  id: string;
  displayName: string;
  ownerId: string;
  isPublic: boolean;
  description: string;
  tagsList: string[];
  collaboratorIdList: string[];
  version: number;
  fileId: string;
  created?: number;
  updated?: number;
}

export interface User {
  id: string;
  email: string;
  displayName: string;
  photoUrl: string;
  provider: string;
  created?: number;
  isAdmin: boolean;
  isDeactivated: boolean;
  canCreateProjects: boolean;
}

export interface Preview {
  id: string;
  png: Uint8Array | string;
  created?: number;
}

export type DataModels = Preview | Project | User | File;

export interface Table<T extends DataModels> {
  findOne(id: string): Promise<T | undefined>;
  findOneByScan(query: Partial<T>): Promise<T | undefined>;
  findByScan(query: Partial<T>): Promise<T[] | undefined>;
  find(idPrefix: string): Promise<T[]>;
  create(id: string, pb: T): Promise<void>;
  update(id: string, cond: Partial<T>, pb: T): Promise<T | null>;
  deleteOne(id: string): Promise<void>;
}
