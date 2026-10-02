// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The two decisions the journey makes about a notebook's kernel, apart from
 * any browser: how its session is started (`startNotebookSession`: one
 * request per attempt, a refusal tried again, a server that refuses every
 * time reported with each refusal's own words) and when a cell run would
 * execute (`kernelState`, `canRunCells`). The server here is a stand-in for
 * `fetch` and the session facts are written out; the journey itself exercises
 * the real ones.
 */

import { test, expect } from '@playwright/test';

import {
  KERNEL_NAME,
  SESSION_START_ATTEMPTS,
  canRunCells,
  kernelState,
  sessionRequestBody,
  startNotebookSession,
  type SessionFacts,
} from './jupyter-server';

const SERVER = { url: 'http://127.0.0.1:1/', token: 'secret' };

type Answer = { status: number; body: string } | { throws: string };

/** A `fetch` that gives `answers` in order and records what it was asked. */
function scripted(answers: Answer[]): {
  post: (url: string, init: RequestInit) => Promise<Response>;
  requests: Array<{ url: string; init: RequestInit }>;
} {
  const requests: Array<{ url: string; init: RequestInit }> = [];
  const post = async (url: string, init: RequestInit): Promise<Response> => {
    const answer = answers[requests.length];
    requests.push({ url, init });
    if (answer === undefined) {
      throw new Error('asked more often than the script answers');
    }
    if ('throws' in answer) {
      throw new Error(answer.throws);
    }
    return new Response(answer.body, { status: answer.status });
  };
  return { post, requests };
}

test('a kernel session is asked for once when the server starts it', async () => {
  const { post, requests } = scripted([{ status: 201, body: '{}' }]);
  await startNotebookSession(SERVER, 'a.ipynb', post);
  expect(requests).toHaveLength(1);
  expect(requests[0].url).toBe('http://127.0.0.1:1/api/sessions');
  expect(requests[0].init.method).toBe('POST');
  expect(requests[0].init.headers).toMatchObject({ Authorization: 'token secret' });
  expect(JSON.parse(String(requests[0].init.body))).toEqual({
    path: 'a.ipynb',
    name: 'a.ipynb',
    type: 'notebook',
    kernel: { name: KERNEL_NAME },
  });
  expect(sessionRequestBody('a.ipynb')).toEqual(JSON.parse(String(requests[0].init.body)));
});

test('a refused start is tried again, whether the server answered or not', async () => {
  for (const refusal of [{ status: 500, body: 'kernel did not start' }, { throws: 'connection reset' }] as Answer[]) {
    const { post, requests } = scripted([refusal, { status: 201, body: '{}' }]);
    await startNotebookSession(SERVER, 'a.ipynb', post);
    expect(requests).toHaveLength(2);
  }
});

test('a server that refuses every attempt fails the start with each refusal', async () => {
  const answers: Answer[] = [];
  for (let i = 0; i < SESSION_START_ATTEMPTS; i++) {
    answers.push({ status: 500, body: `refusal ${i + 1}` });
  }
  const { post, requests } = scripted(answers);
  let message = '';
  try {
    await startNotebookSession(SERVER, 'a.ipynb', post);
  } catch (err) {
    message = err instanceof Error ? err.message : String(err);
  }
  expect(requests).toHaveLength(SESSION_START_ATTEMPTS);
  expect(message).toContain('did not start a kernel session for a.ipynb');
  for (let i = 0; i < SESSION_START_ATTEMPTS; i++) {
    expect(message).toContain(`attempt ${i + 1}: HTTP 500: refusal ${i + 1}`);
  }
});

// Every arm of `kernelState`, in the order it decides them: no notebook, a
// session still starting (whatever its kernel says), no kernel, a kernel whose
// socket is not connected (whatever status it last heard), and a connected
// kernel in each execution state JupyterLab reports.
const KERNEL_STATES: Array<{ facts: SessionFacts | null; state: string; runs: boolean }> = [
  { facts: null, state: 'no notebook', runs: false },
  { facts: { ready: false, connection: null, status: null }, state: 'session starting', runs: false },
  { facts: { ready: false, connection: 'connected', status: 'idle' }, state: 'session starting', runs: false },
  { facts: { ready: true, connection: null, status: null }, state: 'no kernel', runs: false },
  { facts: { ready: true, connection: 'connecting', status: 'idle' }, state: 'connecting', runs: false },
  { facts: { ready: true, connection: 'disconnected', status: 'busy' }, state: 'disconnected', runs: false },
  { facts: { ready: true, connection: 'connected', status: 'unknown' }, state: 'unknown', runs: false },
  { facts: { ready: true, connection: 'connected', status: 'starting' }, state: 'starting', runs: false },
  { facts: { ready: true, connection: 'connected', status: 'restarting' }, state: 'restarting', runs: false },
  { facts: { ready: true, connection: 'connected', status: 'autorestarting' }, state: 'autorestarting', runs: false },
  { facts: { ready: true, connection: 'connected', status: 'terminating' }, state: 'terminating', runs: false },
  { facts: { ready: true, connection: 'connected', status: 'dead' }, state: 'dead', runs: false },
  { facts: { ready: true, connection: 'connected', status: 'idle' }, state: 'idle', runs: true },
  { facts: { ready: true, connection: 'connected', status: 'busy' }, state: 'busy', runs: true },
];

test('a cell run executes only in a started session with a connected, live kernel', () => {
  for (const { facts, state, runs } of KERNEL_STATES) {
    expect(kernelState(facts), JSON.stringify(facts)).toBe(state);
    expect(canRunCells(state), state).toBe(runs);
  }
});
