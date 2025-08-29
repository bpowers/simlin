import { FirebaseApp, initializeApp } from 'firebase/app';
import { connectAuthEmulator, getAuth as _getAuth } from 'firebase/auth';

export const app = initializeApp({
  apiKey: 'AIzaSyConH72HQl9xOtjmYJO9o2kQ9nZZzl96G8',
  authDomain: 'auth.simlin.com',
  projectId: 'test',
});

export async function getAuth(app: FirebaseApp) {
  const auth = _getAuth(app);
  const isDevServer = process.env.NODE_ENV === 'development';
  if (isDevServer) {
    connectAuthEmulator(auth, 'http://localhost:9099', { disableWarnings: true });
  }
  await auth.authStateReady();

  return auth;
}
