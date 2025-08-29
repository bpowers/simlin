import { app, getAuth } from './helpers';

export const firebaseApp = app;

export const auth = await getAuth(firebaseApp);
