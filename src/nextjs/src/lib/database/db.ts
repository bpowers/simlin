import { connectFirestoreEmulator, initializeFirestore } from 'firebase/firestore';
import { app } from '../firebase/helpers';

const firestoreSettings = {};

const db = initializeFirestore(app, firestoreSettings);

const isDevServer = process.env.NODE_ENV === 'development';
if (isDevServer) {
  connectFirestoreEmulator(db, '127.0.0.1', 8080);
}

export default db;
