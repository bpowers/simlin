// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

export interface FirebaseAuthConfig {
  apiKey: string;
  emulatorHost?: string;
}

export interface SignInResponse {
  idToken: string;
  email: string;
  refreshToken: string;
  expiresIn: string;
  localId: string;
  displayName?: string;
}

export interface FetchProvidersResponse {
  providers: string[];
  registered: boolean;
}

export class FirebaseAuthError extends Error {
  constructor(
    public readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'FirebaseAuthError';
  }
}

const ERROR_MESSAGES: Record<string, string> = {
  EMAIL_NOT_FOUND: 'No account found with this email',
  INVALID_PASSWORD: 'Incorrect password',
  EMAIL_EXISTS: 'An account with this email already exists',
  WEAK_PASSWORD: 'Password must be at least 6 characters',
  USER_DISABLED: 'This account has been disabled',
  TOO_MANY_ATTEMPTS_TRY_LATER: 'Too many attempts. Try again later.',
  INVALID_EMAIL: 'Invalid email address',
};

function getErrorCode(rawMessage: string): string {
  const colonIndex = rawMessage.indexOf(':');
  if (colonIndex !== -1) {
    return rawMessage.substring(0, colonIndex).trim();
  }
  return rawMessage;
}

function parseErrorMessage(rawMessage: string): { code: string; message: string } {
  const code = getErrorCode(rawMessage);
  const message = ERROR_MESSAGES[code] ?? rawMessage;
  return { code, message };
}

export interface FirebaseRestClient {
  signInWithPassword(email: string, password: string): Promise<SignInResponse>;
  signUp(email: string, password: string, displayName?: string): Promise<SignInResponse>;
  fetchProviders(email: string, continueUri: string): Promise<FetchProvidersResponse>;
  sendPasswordResetEmail(email: string): Promise<void>;
}

export function createFirebaseRestClient(config: FirebaseAuthConfig): FirebaseRestClient {
  const baseUrl = config.emulatorHost
    ? `http://${config.emulatorHost}/identitytoolkit.googleapis.com/v1`
    : 'https://identitytoolkit.googleapis.com/v1';

  async function request<T>(endpoint: string, body: object): Promise<T> {
    const url = `${baseUrl}/${endpoint}?key=${config.apiKey}`;
    const response = await fetch(url, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(body),
    });

    const data = await response.json();

    if (!response.ok) {
      const rawMessage = data.error?.message ?? 'Unknown error';
      const { code, message } = parseErrorMessage(rawMessage);
      throw new FirebaseAuthError(code, message);
    }

    return data as T;
  }

  return {
    async signInWithPassword(email: string, password: string): Promise<SignInResponse> {
      return request<SignInResponse>('accounts:signInWithPassword', {
        email,
        password,
        returnSecureToken: true,
      });
    },

    async signUp(email: string, password: string, displayName?: string): Promise<SignInResponse> {
      const body: Record<string, unknown> = {
        email,
        password,
        returnSecureToken: true,
      };
      if (displayName) {
        body.displayName = displayName;
      }
      return request<SignInResponse>('accounts:signUp', body);
    },

    async fetchProviders(email: string, continueUri: string): Promise<FetchProvidersResponse> {
      interface RawResponse {
        registered?: boolean;
        allProviders?: string[];
      }
      const data = await request<RawResponse>('accounts:createAuthUri', {
        identifier: email,
        continueUri,
      });
      return {
        registered: data.registered ?? false,
        providers: data.allProviders ?? [],
      };
    },

    async sendPasswordResetEmail(email: string): Promise<void> {
      try {
        await request('accounts:sendOobCode', {
          requestType: 'PASSWORD_RESET',
          email,
        });
      } catch (err) {
        if (err instanceof FirebaseAuthError && err.code === 'EMAIL_NOT_FOUND') {
          return;
        }
        throw err;
      }
    },
  };
}
