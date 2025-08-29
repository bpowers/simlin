import {
  GoogleAuthProvider,
  fetchSignInMethodsForEmail as _fetchSignInMethodsForEmail,
  onAuthStateChanged as _onAuthStateChanged,
  onIdTokenChanged as _onIdTokenChanged,
  sendPasswordResetEmail as _sendPasswordResetEmail,
  signInWithEmailAndPassword,
  createUserWithEmailAndPassword,
  NextOrObserver,
  User,
  OAuthProvider,
  signInWithRedirect,
} from 'firebase/auth';

import { auth } from '@/lib/firebase/clientApp';

export function fetchSignInMethodsForEmail(email: string) {
  return _fetchSignInMethodsForEmail(auth, email);
}

export function sendPasswordResetEmail(email: string) {
  return _sendPasswordResetEmail(auth, email);
}

export function onAuthStateChanged(cb: NextOrObserver<User>) {
  return _onAuthStateChanged(auth, cb);
}

export function onIdTokenChanged(cb: NextOrObserver<User>) {
  return _onIdTokenChanged(auth, cb);
}

export async function signInWithGoogle() {
  const provider = new GoogleAuthProvider();
  provider.addScope('profile');

  await signInWithRedirect(auth, provider);
}

export async function signInWithApple() {
  const provider = new OAuthProvider('apple.com');
  provider.addScope('email');
  provider.addScope('name');

  await signInWithRedirect(auth, provider);
}

export async function signInWithEmail(email: string, password: string) {
  return await signInWithEmailAndPassword(auth, email, password);
}

export async function signUpWithEmail(email: string, password: string) {
  return await createUserWithEmailAndPassword(auth, email, password);
}

export async function signOut() {
  return await auth.signOut();
}
