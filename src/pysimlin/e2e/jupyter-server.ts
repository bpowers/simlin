// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Launching and stopping a throwaway JupyterLab server for the notebook
 * journey.  The server runs from the pysimlin venv's interpreter (so its
 * kernel imports the checkout's `simlin` and its `share/jupyter` carries the
 * anywidget and ipywidgets labextensions), on an OS-assigned port, with a
 * random token, and with every Jupyter directory (config, data, runtime,
 * root) pointed at a fresh temporary tree so nothing from `~/.jupyter` or an
 * earlier run leaks in.
 */

import * as child_process from 'node:child_process';
import * as crypto from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

export interface ServerInfo {
  /** Base URL with a trailing slash, e.g. `http://127.0.0.1:41234/`. */
  url: string;
  token: string;
  rootDir: string;
}

export interface LaunchedServer extends ServerInfo {
  /** Working tree; removed by `stop`. */
  tmpDir: string;
  logPath: string;
  stop(): Promise<void>;
}

/** Environment variable names the setup hands to the workers. */
export const ENV = {
  python: 'SIMLIN_E2E_PYTHON',
  url: 'SIMLIN_E2E_JUPYTER_URL',
  token: 'SIMLIN_E2E_JUPYTER_TOKEN',
  rootDir: 'SIMLIN_E2E_ROOT_DIR',
} as const;

const here = __dirname;
export const pysimlinDir = path.resolve(here, '..');

/** The interpreter of a venv with pysimlin (and its `e2e` extra) installed. */
export function pythonExecutable(): string {
  return process.env[ENV.python] ?? path.join(pysimlinDir, '.venv', 'bin', 'python');
}

/**
 * The `jupyter lab` command line.  Config traits are pinned rather than
 * inherited: the token is what the browser authenticates with; the update
 * check and news feed are network calls that would fail (slowly) offline
 * and produce nothing the test uses; the extension manager set to readonly
 * keeps a stray click from reaching PyPI.
 */
export function serverArgs(rootDir: string, token: string): string[] {
  return [
    '-m',
    'jupyterlab',
    '--no-browser',
    '--port=0',
    '--ip=127.0.0.1',
    `--IdentityProvider.token=${token}`,
    `--ServerApp.root_dir=${rootDir}`,
    '--LabApp.check_for_updates_class=jupyterlab.handlers.announcements.NeverCheckForUpdate',
    '--LabApp.news_url=None',
    '--LabApp.extension_manager=readonly',
  ];
}

/**
 * User settings written into the run's config tree before launch.  The
 * server-side `news_url=None` stops the fetch, but the front end still asks
 * "Would you like to get notified about official Jupyter news?" until the
 * user answers, and that toast sits over the notebook in every screenshot;
 * answering it in the settings file (`fetchNews: "false"`, the persisted
 * form of clicking No) is what JupyterLab documents for headless installs.
 * The path is under `JUPYTER_CONFIG_DIR/lab/user-settings`, LabApp's default
 * `user_settings_dir`, so nothing outside the temporary tree is touched.
 */
export function userSettingsFiles(configDir: string): Array<{ file: string; contents: string }> {
  const settingsDir = path.join(configDir, 'lab', 'user-settings');
  return [
    {
      file: path.join(settingsDir, '@jupyterlab', 'apputils-extension', 'notification.jupyterlab-settings'),
      contents: JSON.stringify({ fetchNews: 'false' }, null, 2) + '\n',
    },
  ];
}

/** Read `url`/`token`/`root_dir` from a `jpserver-<pid>.json` runtime file. */
export function parseServerInfo(json: string): ServerInfo {
  const info = JSON.parse(json) as { url?: unknown; token?: unknown; root_dir?: unknown };
  if (typeof info.url !== 'string' || typeof info.token !== 'string' || typeof info.root_dir !== 'string') {
    throw new Error(`unexpected jupyter server info: ${json}`);
  }
  return { url: info.url.endsWith('/') ? info.url : `${info.url}/`, token: info.token, rootDir: info.root_dir };
}

/**
 * Jupyter's own directories for this run: the runtime dir is where the
 * server drops the `jpserver-<pid>.json` we read the port and token from.
 */
export function jupyterEnv(tmpDir: string): Record<string, string> {
  return {
    JUPYTER_CONFIG_DIR: path.join(tmpDir, 'config'),
    JUPYTER_DATA_DIR: path.join(tmpDir, 'data'),
    JUPYTER_RUNTIME_DIR: path.join(tmpDir, 'runtime'),
    // Cleared, not merely unset-by-omission: the child inherits process.env,
    // and any of these would pull extensions, config, or a lab build from
    // outside the venv into the run.
    JUPYTER_PATH: '',
    JUPYTER_CONFIG_PATH: '',
    JUPYTERLAB_DIR: '',
    // The kernel must not inherit a widget-asset override from the shell
    // running the test: the journey covers the default (bundled) delivery.
    SIMLIN_WIDGET_ASSET: '',
  };
}

async function waitFor<T>(what: string, timeoutMs: number, probe: () => Promise<T | undefined>): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await probe();
    if (value !== undefined) {
      return value;
    }
    if (Date.now() > deadline) {
      throw new Error(`timed out after ${timeoutMs} ms waiting for ${what}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

/**
 * Fail before launching anything if the interpreter cannot import simlin
 * with its widget assets and jupyterlab: the same failures inside a cell
 * surface as a traceback in a screenshot, which is a slower way to learn
 * that the extension is unbuilt or `simlin/_widget/` is empty.
 */
export function preflight(python: string): void {
  if (!fs.existsSync(python)) {
    throw new Error(
      `${python} does not exist; create the pysimlin venv with the e2e extra ` +
        `(cd src/pysimlin && uv sync --extra dev --extra e2e) or point ${ENV.python} at an interpreter`,
    );
  }
  const script = [
    'import simlin, simlin.widget, jupyterlab',
    'assets = simlin.widget._ASSETS',
    'if assets.esm is None or assets.wasm_path is None:',
    '    raise SystemExit(assets.error or "widget assets missing from simlin/_widget/")',
  ].join('\n');
  const result = child_process.spawnSync(python, ['-c', script], {
    encoding: 'utf8',
    env: { ...process.env, SIMLIN_WIDGET_ASSET: '' },
  });
  if (result.status !== 0) {
    throw new Error(`${python} cannot run the notebook journey:\n${result.stderr}${result.stdout}`);
  }
}

/** Launch JupyterLab and resolve once its REST API answers with our token. */
export async function launchJupyterLab(): Promise<LaunchedServer> {
  const python = pythonExecutable();
  preflight(python);
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'simlin-e2e-'));
  const rootDir = path.join(tmpDir, 'root');
  const env = jupyterEnv(tmpDir);
  for (const dir of [rootDir, env.JUPYTER_CONFIG_DIR, env.JUPYTER_DATA_DIR, env.JUPYTER_RUNTIME_DIR]) {
    fs.mkdirSync(dir, { recursive: true });
  }
  for (const { file, contents } of userSettingsFiles(env.JUPYTER_CONFIG_DIR)) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, contents);
  }
  const token = crypto.randomBytes(24).toString('hex');
  const logPath = path.join(tmpDir, 'jupyter-lab.log');
  const log = fs.openSync(logPath, 'w');
  const child = child_process.spawn(python, serverArgs(rootDir, token), {
    env: { ...process.env, ...env },
    stdio: ['ignore', log, log],
    // Its own process group so a stop can take the kernels down with it.
    detached: true,
  });
  let exited = false;
  child.on('exit', () => {
    exited = true;
  });

  // Whatever the server's state, take it down (the whole process group, so
  // kernels go with it) and drop the temporary tree; the log is read before
  // the tree goes so a failure report can carry it.
  const killAndClean = (): void => {
    if (!exited && child.pid !== undefined) {
      try {
        process.kill(-child.pid, 'SIGKILL');
      } catch {
        // Already gone.
      }
    }
    fs.closeSync(log);
    if (process.env.SIMLIN_E2E_KEEP_TMP === undefined) {
      fs.rmSync(tmpDir, { recursive: true, force: true });
    }
  };

  let info: ServerInfo;
  try {
    const runtimeDir = env.JUPYTER_RUNTIME_DIR;
    info = await waitFor('the jupyter runtime file', 60_000, async () => {
      if (exited) {
        throw new Error(`jupyter lab exited during startup; see ${logPath}:\n${fs.readFileSync(logPath, 'utf8')}`);
      }
      const files = fs.readdirSync(runtimeDir).filter((f) => f.startsWith('jpserver-') && f.endsWith('.json'));
      if (files.length === 0) {
        return undefined;
      }
      return parseServerInfo(fs.readFileSync(path.join(runtimeDir, files[0]), 'utf8'));
    });
    const url = info.url;
    const bearer = info.token;
    await waitFor('the jupyter REST API', 60_000, async () => {
      try {
        const res = await fetch(`${url}api/status`, { headers: { Authorization: `token ${bearer}` } });
        return res.ok ? true : undefined;
      } catch {
        return undefined;
      }
    });
  } catch (err) {
    // A startup failure must not leave a detached jupyter (and its
    // kernels) running or a temporary tree behind; the error already
    // carries the log where it matters (exit during startup), and a
    // timeout gets the log appended here before the tree is removed.
    const message = err instanceof Error ? err.message : String(err);
    const logText = message.includes(logPath) ? '' : `\n${logPath}:\n${fs.readFileSync(logPath, 'utf8')}`;
    killAndClean();
    throw new Error(`${message}${logText}`);
  }

  const stop = async (): Promise<void> => {
    if (!exited) {
      // Ask nicely first so the server shuts its kernels down itself.
      try {
        await fetch(`${info.url}api/shutdown`, {
          method: 'POST',
          headers: { Authorization: `token ${info.token}` },
        });
      } catch {
        // Falls through to the signal.
      }
      const deadline = Date.now() + 15_000;
      while (!exited && Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    }
    killAndClean();
  };

  return { ...info, tmpDir, logPath, stop };
}
