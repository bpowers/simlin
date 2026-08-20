// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The JupyterLab journey (design AC4.2, exercising AC2.1-2.3 and AC2.7):
 * a notebook opens a model file with `simlin.open`, displays it, and the
 * cell shows the real Editor; a variable added and given an equation in the
 * Editor lands in the file on disk and in the kernel's model (`revision`,
 * `get_var_names`, `run`); a Python `edit()` and an external write to the
 * file both reach the Editor with their notices.  Everything runs against a
 * real `jupyter lab` (see global-setup) with a real ipykernel: no fakes on
 * either side of the comm.
 *
 * The second test is the other headline path (both example notebooks open
 * with it): a model built from scratch with `Project.new()` + `edit()`,
 * which has no diagram view until it is displayed -- the display must lay
 * it out (a blank Editor is what a viewless model renders as) and a Python
 * edit afterwards must keep that in-memory diagram in step.
 *
 * What it does NOT establish: behaviour in Colab or VS Code (manual
 * checklists, AC2.8), or the bundle-level invariants (nothing fetched
 * relative to the module, wasm shared across cells) that
 * src/notebook-widget/e2e pins with a fake model.
 */

import * as child_process from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';

import { test, expect, type Locator, type Page } from '@playwright/test';

import { ENV, pysimlinDir, pythonExecutable } from './jupyter-server';

const NOTEBOOK = 'journey.ipynb';
const MODEL = 'teacup.xmile';
const FIXTURE = path.join(pysimlinDir, 'tests', 'fixtures', MODEL);

// The cells the notebook starts with; the test runs them one at a time,
// interleaved with Editor interaction, and reads their printed output.
const CELLS = [
  ['import simlin', `m = simlin.open("${MODEL}")`, 'm'],
  ['print("REV", m.revision, sorted(m.get_var_names()))', 'print("RUN", sorted(m.run().results.columns))'],
  ['print("SEL", m.selection)'],
  [
    'with m.edit() as (_, p):',
    '    p.upsert(simlin.Aux(name="from_python", equation="1"))',
    'print("REV", m.revision)',
  ],
  ['print("REV", m.revision, sorted(m.get_var_names()))', 'print("SEL", m.selection)'],
];

const SCRATCH_NOTEBOOK = 'scratch.ipynb';
const SCRATCH_CELLS = [
  [
    'import simlin',
    'project = simlin.Project.new(name="scratch")',
    'm = project.get_model()',
    'with m.edit() as (_, p):',
    '    p.upsert(simlin.Stock(name="population", initial_equation="100", inflows=["births"]))',
    '    p.upsert(simlin.Flow(name="births", equation="population * birth_rate"))',
    '    p.upsert(simlin.Aux(name="birth_rate", equation="0.02"))',
    'print("REV", m.revision)',
    'm',
  ],
  [
    'with m.edit() as (_, p):',
    '    p.upsert(simlin.Aux(name="from_python", equation="1"))',
    'print("REV", m.revision)',
  ],
];

function notebookJson(cells: string[][]): string {
  return JSON.stringify(
    {
      cells: cells.map((lines, i) => ({
        cell_type: 'code',
        execution_count: null,
        id: `cell-${i}`,
        metadata: {},
        outputs: [],
        source: lines.join('\n'),
      })),
      metadata: { kernelspec: { display_name: 'Python 3', language: 'python', name: 'python3' } },
      nbformat: 4,
      nbformat_minor: 5,
    },
    null,
    1,
  );
}

function required(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`${name} is not set; global-setup did not run`);
  }
  return value;
}

/**
 * The cells of the ACTIVE notebook.  JupyterLab restores the workspace, so
 * a second test finds the first test's notebook still open in a hidden tab
 * (Lumino marks inactive dock tabs `lm-mod-hidden`); an unscoped `.jp-Cell`
 * would count both notebooks' cells.
 */
function activeCells(page: Page): Locator {
  return page.locator('.jp-NotebookPanel:not(.lm-mod-hidden) .jp-Notebook .jp-Cell');
}

/**
 * Wait until the visible notebook has a live kernel. On a cold server (the
 * first notebook opened after Lab's caches were rebuilt) Lab can still be
 * starting the session when the page shows its cells; a Shift+Enter then
 * opens the "Select Kernel" dialog instead of running the cell. The toolbar's
 * kernel-name button is no signal (it shows the preferred name before a
 * session exists); the execution indicator reports `data-status="idle"` only
 * once a kernel is connected. A "Select Kernel" dialog left open is accepted
 * so the default kernel starts.
 */
async function waitForKernel(page: Page): Promise<void> {
  const dialogSelect = page.locator('.jp-Dialog button.jp-mod-accept', { hasText: /^select$/i });
  const indicator = page.locator('.jp-NotebookPanel:not(.lm-mod-hidden) .jp-Notebook-ExecutionIndicator');
  await expect
    .poll(
      async () => {
        if (await dialogSelect.isVisible()) {
          await dialogSelect.click();
        }
        return indicator.first().getAttribute('data-status');
      },
      { timeout: 90_000, message: 'waiting for the notebook kernel to connect' },
    )
    .toMatch(/^(idle|busy)$/);
}

/** Click into cell `index`'s editor and run it with Shift+Enter. */
async function runCell(page: Page, index: number): Promise<Locator> {
  await waitForKernel(page);
  const cell = activeCells(page).nth(index);
  await cell.locator('.jp-InputArea-editor').click();
  await page.keyboard.press('Shift+Enter');
  return cell;
}

/** The text of every output of a cell, once it has produced at least one. */
async function cellOutput(cell: Locator, contains: string | RegExp): Promise<string> {
  const outputs = cell.locator('.jp-OutputArea-output');
  await expect(outputs).toContainText([contains]);
  return (await outputs.allTextContents()).join('\n');
}

function revisionIn(output: string): number {
  const m = /REV (\d+)/.exec(output);
  if (m === null) {
    throw new Error(`no revision in cell output: ${output}`);
  }
  return Number(m[1]);
}

/** Read the model file until `predicate` holds (the Editor writes through the kernel asynchronously). */
async function waitForFile(file: string, predicate: (text: string) => boolean): Promise<string> {
  let latest = '';
  await expect
    .poll(
      () => {
        latest = fs.readFileSync(file, 'utf8');
        return predicate(latest);
      },
      { timeout: 30_000, message: `waiting for ${file} to change` },
    )
    .toBe(true);
  return latest;
}

/**
 * The position of a diagram element RELATIVE TO ITS CANVAS, once it has
 * stopped moving (a pan may still be coasting). Relative, because bounding
 * boxes are viewport coordinates and running a cell scrolls the notebook.
 */
async function settledOffset(target: Locator, canvas: Locator): Promise<{ x: number; y: number }> {
  const read = async (): Promise<{ x: number; y: number } | null> => {
    const [box, frame] = await Promise.all([target.boundingBox(), canvas.boundingBox()]);
    return box === null || frame === null ? null : { x: box.x - frame.x, y: box.y - frame.y };
  };
  let previous = await read();
  await expect
    .poll(
      async () => {
        const next = await read();
        const settled =
          previous !== null &&
          next !== null &&
          Math.abs(next.x - previous.x) < 0.01 &&
          Math.abs(next.y - previous.y) < 0.01;
        previous = next;
        return settled;
      },
      { timeout: 10_000, intervals: [250], message: 'waiting for the element to stop moving' },
    )
    .toBe(true);
  if (previous === null) {
    throw new Error('the element has no bounding box');
  }
  return previous;
}

/**
 * Shift-drag on empty canvas pans the diagram (Canvas: `e.shiftKey` selects
 * pan over rubber-band selection). Two legs so the gesture is a clear drag,
 * and no flick at the end so it settles without a momentum coast.
 */
async function shiftPan(
  page: Page,
  canvas: Locator,
  from: { x: number; y: number },
  by: { dx: number; dy: number },
): Promise<void> {
  const box = await canvas.boundingBox();
  if (box === null) {
    throw new Error('the Editor canvas has no bounding box');
  }
  const startX = box.x + from.x;
  const startY = box.y + from.y;
  await page.keyboard.down('Shift');
  await page.mouse.move(startX, startY);
  await page.mouse.down();
  await page.mouse.move(startX + by.dx / 2, startY + by.dy / 2, { steps: 8 });
  await page.mouse.move(startX + by.dx, startY + by.dy, { steps: 8 });
  await page.waitForTimeout(150);
  await page.mouse.up();
  await page.keyboard.up('Shift');
}

test('pysimlin-widget.AC4.2: JupyterLab notebook edits a model file through the Editor and follows changes to it', async ({
  page,
}) => {
  const baseUrl = required(ENV.url);
  const token = required(ENV.token);
  const rootDir = required(ENV.rootDir);
  const modelPath = path.join(rootDir, MODEL);
  fs.copyFileSync(FIXTURE, modelPath);
  fs.writeFileSync(path.join(rootDir, NOTEBOOK), notebookJson(CELLS));
  const original = fs.readFileSync(modelPath, 'utf8');

  const consoleErrors: string[] = [];
  page.on('pageerror', (err) => consoleErrors.push(`pageerror: ${err.message}`));
  page.on('console', (msg) => {
    if (msg.type() === 'error') {
      consoleErrors.push(`console.error: ${msg.text()}`);
    }
  });

  await page.goto(`${baseUrl}lab/tree/${NOTEBOOK}?token=${token}`);
  await expect(activeCells(page)).toHaveCount(CELLS.length, { timeout: 90_000 });

  // --- display: the Editor renders in the cell output -------------------
  const displayCell = await runCell(page, 0);
  const widget = displayCell.locator('.simlin-notebook-widget');
  await expect(widget).toBeVisible({ timeout: 90_000 });
  // The diagram canvas (not the first <svg>: the search bar's icons come
  // before it in the DOM).
  const canvas = widget.locator('svg.simlin-canvas');
  await expect(canvas).toBeVisible({ timeout: 60_000 });
  await expect(widget.getByText('Teacup Temperature', { exact: true }).first()).toBeVisible();

  // --- edit in the Editor: add a variable, give it an equation ----------
  await widget.getByRole('button', { name: 'hide or show editor tools' }).click();
  await widget.getByRole('button', { name: 'Variable', exact: true }).click();
  const box = await canvas.boundingBox();
  if (box === null) {
    throw new Error('the Editor canvas has no bounding box');
  }
  // Empty canvas: clear of the tool dial (bottom left), the search bar (top
  // right), and the teacup's elements (centred).
  await page.mouse.click(box.x + 120, box.y + 120);
  // The inline name editor opens with the default name selected; accept it.
  await page.keyboard.press('Enter');
  const label = canvas.locator('text', { hasText: 'New Variable' }).first();
  await expect(label).toBeVisible();
  // The creation was written straight through: an aux with an empty
  // equation is a valid (if not yet simulatable) model, so the file changed
  // before we touch the equation.
  await waitForFile(modelPath, (text) => text !== original && /New[ _]Variable/.test(text));

  // Put the creation tool away, then a click (no drag) on the variable's
  // circle opens its details (a click on the label would start renaming
  // it).  Clicking the rendered-equation preview swaps in the raw editor.
  // The details panel's classes are CSS-module names, `<local>-<hash>`.
  await widget.getByRole('button', { name: 'Variable', exact: true }).click();
  await canvas.locator('g.simlin-aux', { hasText: 'New Variable' }).locator('circle').first().click();
  await widget.locator('[class*="eqnPreview"]').click();
  const equationEditor = widget.locator('[data-slate-editor="true"][class*="eqnEditor"]');
  await expect(equationEditor).toBeVisible();
  await equationEditor.click();
  await page.keyboard.type('72');
  await widget.getByRole('button', { name: 'Save', exact: true }).click();
  const withEquation = await waitForFile(modelPath, (text) => text.includes('<eqn>72</eqn>'));
  expect(withEquation).toMatch(/New[ _]Variable/);

  // --- the kernel sees it: revision, names, and a run -------------------
  const afterEditor = await cellOutput(await runCell(page, 1), 'RUN');
  // Exactly two accepted snapshots so far: the creation (name accepted) and
  // the equation save. Placing the element and opening its details do not
  // change the project, and the Editor sends one snapshot per accepted edit.
  expect(revisionIn(afterEditor)).toBe(2);
  expect(afterEditor).toContain("'new_variable'");
  const runLine = afterEditor.split('\n').find((line) => line.startsWith('RUN'));
  expect(runLine).toContain("'new_variable'");
  const revisionAfterEditor = revisionIn(afterEditor);

  // --- AC2.7: the selection is readable from Python ---------------------
  const selection = await cellOutput(await runCell(page, 2), 'SEL');
  expect(selection).toContain("('new_variable',)");

  // --- a pan is not lost to a Python edit -------------------------------
  // A pan alone is never saved (only the next edit's save carries the
  // viewport), and a Python edit remounts the Editor on the kernel's bytes;
  // the widget carries the live viewport across that remount, so the stock
  // stays exactly where the user put it. Press on empty canvas (the top-left
  // corner: clear of the search bar top-right, the tool dial bottom-left, the
  // teacup's centred elements and the variable added at 120,120), then verify
  // the stock actually moved on screen and record where it settled.
  const stock = canvas.locator('g.simlin-stock', { hasText: /teacup\s*temperature/i }).first();
  const beforePan = await settledOffset(stock, canvas);
  await shiftPan(page, canvas, { x: 40, y: 40 }, { dx: 90, dy: 60 });
  const afterPan = await settledOffset(stock, canvas);
  expect(Math.hypot(afterPan.x - beforePan.x, afterPan.y - beforePan.y)).toBeGreaterThan(40);
  // The pan by itself wrote nothing (the file still has exactly the two
  // Editor edits) -- which is precisely why it would be lost without the carry.
  expect(fs.readFileSync(modelPath, 'utf8')).toBe(withEquation);

  // --- a Python edit reaches the Editor ---------------------------------
  const notice = widget.getByRole('status');
  const afterPython = await cellOutput(await runCell(page, 3), 'REV');
  expect(revisionIn(afterPython)).toBe(revisionAfterEditor + 1);
  await expect(notice).toHaveText('Updated from Python');
  // Kernel-added variables get a diagram element from the incremental
  // layout, whose label breaks long names across lines (so the label's
  // text content may run the words together).
  await expect(canvas.locator('g.simlin-aux', { hasText: /from\s*python/ })).toBeVisible();
  // The remounted Editor opened on the carried viewport: the stock is where
  // the pan left it, to the pixel.
  const afterPythonPos = await settledOffset(
    canvas.locator('g.simlin-stock', { hasText: /teacup\s*temperature/i }).first(),
    canvas,
  );
  expect(Math.abs(afterPythonPos.x - afterPan.x)).toBeLessThanOrEqual(1);
  expect(Math.abs(afterPythonPos.y - afterPan.y)).toBeLessThanOrEqual(1);

  // --- an external write to the file reaches the Editor and the kernel --
  const script = [
    'import simlin',
    `m = simlin.open(${JSON.stringify(modelPath)}, watch=False)`,
    'with m.edit() as (_, p):',
    '    p.upsert(simlin.Aux(name="external_writer", equation="4"))',
  ].join('\n');
  child_process.execFileSync(pythonExecutable(), ['-c', script], { stdio: 'pipe' });
  await expect(notice).toHaveText('Updated on disk');
  await expect(canvas.locator('g.simlin-aux', { hasText: /external\s*writer/ })).toBeVisible();
  const afterExternal = await cellOutput(await runCell(page, 4), 'REV');
  expect(revisionIn(afterExternal)).toBe(revisionAfterEditor + 2);
  expect(afterExternal).toContain("'external_writer'");
  expect(afterExternal).toContain("'from_python'");
  expect(afterExternal).toContain("'new_variable'");
  // AC2.7, the other direction: the kernel pushes (the Python edit, the disk
  // change) remounted the Editor, which starts with nothing selected, and the
  // widget published that -- `m.selection` no longer names `new_variable`.
  expect(afterExternal).toContain('SEL ()');

  // Kept as visual artifacts of the run (gitignored), not assertions.
  await page.screenshot({ path: path.join(__dirname, '.output', 'journey-notebook.png') });
  await widget.scrollIntoViewIfNeeded();
  await widget.screenshot({ path: path.join(__dirname, '.output', 'journey-widget.png') });

  // --- nothing went wrong along the way ---------------------------------
  await expect(page.locator('.jp-OutputArea-output[data-mime-type="application/vnd.jupyter.error"]')).toHaveCount(0);
  await expect(page.locator('.jp-OutputArea-output[data-mime-type="application/vnd.jupyter.stderr"]')).toHaveCount(0);
  expect(consoleErrors).toEqual([]);
});

test('a model built from scratch in memory displays with a laid-out diagram and follows Python edits', async ({
  page,
}) => {
  const baseUrl = required(ENV.url);
  const token = required(ENV.token);
  const rootDir = required(ENV.rootDir);
  fs.writeFileSync(path.join(rootDir, SCRATCH_NOTEBOOK), notebookJson(SCRATCH_CELLS));

  const consoleErrors: string[] = [];
  page.on('pageerror', (err) => consoleErrors.push(`pageerror: ${err.message}`));
  page.on('console', (msg) => {
    if (msg.type() === 'error') {
      consoleErrors.push(`console.error: ${msg.text()}`);
    }
  });

  await page.goto(`${baseUrl}lab/tree/${SCRATCH_NOTEBOOK}?token=${token}`);
  await expect(activeCells(page)).toHaveCount(SCRATCH_CELLS.length, { timeout: 90_000 });

  // The display lays the viewless model out (one committed change, so the
  // revision printed BEFORE the display is one behind what the next cell
  // sees) and the Editor shows every variable, not a blank canvas.
  const displayCell = await runCell(page, 0);
  const before = await cellOutput(displayCell, 'REV');
  expect(revisionIn(before)).toBe(1);
  const widget = displayCell.locator('.simlin-notebook-widget');
  await expect(widget).toBeVisible({ timeout: 90_000 });
  const canvas = widget.locator('svg.simlin-canvas');
  await expect(canvas).toBeVisible({ timeout: 60_000 });
  await expect(canvas.locator('g.simlin-stock', { hasText: /population/i })).toBeVisible();
  await expect(canvas.locator('g.simlin-aux', { hasText: /birth\s*rate/i })).toBeVisible();

  // A laid-out-on-display model has no stored viewport yet (0/0/0/0 until a
  // browser edit saves one), so the canvas fits it on mount; that fitted
  // framing is what the user is looking at, and a remount must not re-centre
  // on the grown diagram under them. Record where the stock is.
  const stock = canvas.locator('g.simlin-stock', { hasText: /population/i }).first();
  const beforePython = await settledOffset(stock, canvas);

  // An in-memory model that has a diagram keeps it in step: the Python
  // edit's variable gets an element and the notice names the source.
  const notice = widget.getByRole('status');
  const afterPython = await cellOutput(await runCell(page, 1), 'REV');
  expect(revisionIn(afterPython)).toBe(3);
  await expect(notice).toHaveText('Updated from Python');
  await expect(canvas.locator('g.simlin-aux', { hasText: /from\s*python/ })).toBeVisible();
  // ...and the diagram did not shift: the widget carried the fitted viewport
  // across the remount instead of letting the mount fit re-centre it.
  const afterPythonPos = await settledOffset(
    canvas.locator('g.simlin-stock', { hasText: /population/i }).first(),
    canvas,
  );
  expect(Math.abs(afterPythonPos.x - beforePython.x)).toBeLessThanOrEqual(1);
  expect(Math.abs(afterPythonPos.y - beforePython.y)).toBeLessThanOrEqual(1);

  await expect(page.locator('.jp-OutputArea-output[data-mime-type="application/vnd.jupyter.error"]')).toHaveCount(0);
  await expect(page.locator('.jp-OutputArea-output[data-mime-type="application/vnd.jupyter.stderr"]')).toHaveCount(0);
  expect(consoleErrors).toEqual([]);
});

// AC2.6 in a real JupyterLab: keys typed with the Editor focused act on the
// Editor and never on the notebook. Two mechanisms are under test, and only
// a real Lab can exercise them: (1) a pointer press anywhere inside the
// widget -- a variable's circle included, whose pointerdown is
// preventDefault()ed by the canvas -- must move focus INTO the widget,
// otherwise focus stays on the notebook cell and Lab's command-mode keys
// (`x`, `d d`, `a`) act on cells while Delete reaches no Editor; (2) Lumino
// matches its keybinding selectors (`.jp-Notebook.jp-mod-commandMode:not(.jp-mod-readWrite) :focus`)
// against the focused element BEFORE walking up to any ancestor carrying
// `data-lm-suppress-shortcuts`, so the attribute has to be on the focused
// element itself, not only on the wrapper.
test('pysimlin-widget.AC2.6: keys with the Editor focused act on the Editor, not the notebook', async ({ page }) => {
  const baseUrl = required(ENV.url);
  const token = required(ENV.token);
  const rootDir = required(ENV.rootDir);
  const model = 'keys.xmile';
  const modelPath = path.join(rootDir, model);
  fs.copyFileSync(FIXTURE, modelPath);
  const notebook = 'keys.ipynb';
  const cellSources = [['import simlin', `m = simlin.open("${model}")`, 'm'], ['print(1)'], ['print(2)']];
  fs.writeFileSync(path.join(rootDir, notebook), notebookJson(cellSources));

  await page.goto(`${baseUrl}lab/tree/${notebook}?token=${token}`);
  // Lab restores the workspace, so notebooks from earlier tests are open in
  // other (hidden) tabs: scope to the visible notebook panel.
  const panel = page.locator('.jp-NotebookPanel:not(.lm-mod-hidden)');
  const cells = panel.locator('.jp-Notebook .jp-Cell');
  await expect(cells).toHaveCount(cellSources.length, { timeout: 90_000 });
  const displayCell = await runCell(page, 0);
  const widget = displayCell.locator('.simlin-notebook-widget');
  const canvas = widget.locator('svg.simlin-canvas');
  await expect(canvas).toBeVisible({ timeout: 90_000 });

  const focusInWidget = (): Promise<{ inWidget: boolean; suppressed: boolean; tag: string }> =>
    page.evaluate(() => {
      const a = document.activeElement;
      return {
        inWidget: a !== null && a.closest('.simlin-notebook-widget') !== null,
        suppressed: a !== null && a.hasAttribute('data-lm-suppress-shortcuts'),
        tag: a === null ? 'none' : a.tagName,
      };
    });

  // A click on a variable's circle (a preventDefault()ed canvas press)
  // selects it and moves focus into the widget, onto an element Lumino will
  // not match its notebook shortcuts against.
  const auxes = canvas.locator('g.simlin-aux');
  const auxCount = await auxes.count();
  expect(auxCount).toBeGreaterThan(0);
  const target = auxes.filter({ hasText: /room/i }).first();
  await target.locator('circle').first().click();
  await expect(target).toHaveClass(/simlin-selected/);
  expect(await focusInWidget()).toMatchObject({ inWidget: true, suppressed: true });

  // Lab command-mode keys do nothing to the notebook: `d d` would delete the
  // active cell, `x` cut it, `a` insert one above.
  await page.keyboard.press('d');
  await page.keyboard.press('d');
  await page.keyboard.press('x');
  await page.keyboard.press('a');
  // Give Lab a moment to have acted, then assert it did not.
  await page.waitForTimeout(500);
  await expect(cells).toHaveCount(cellSources.length);
  await expect(canvas).toBeVisible();

  // The Editor's own key acts: Delete removes the selected variable and the
  // file follows.
  await page.keyboard.press('Delete');
  await expect(auxes).toHaveCount(auxCount - 1);
  await expect(target).toHaveCount(0);
  await waitForFile(modelPath, (text) => !/name="Room Temperature"/.test(text));

  // Focus is still inside the widget after the edit; a Lab key still does
  // nothing to the notebook.
  expect(await focusInWidget()).toMatchObject({ inWidget: true, suppressed: true });
  await page.keyboard.press('b');
  await page.waitForTimeout(500);
  await expect(cells).toHaveCount(cellSources.length);

  // And a click on the notebook outside the widget hands the keys back to Lab:
  // `b` in command mode inserts a cell below.
  await cells.nth(2).locator('.jp-InputArea-editor').click();
  await page.keyboard.press('Escape');
  await page.keyboard.press('b');
  await expect(cells).toHaveCount(cellSources.length + 1);
});
