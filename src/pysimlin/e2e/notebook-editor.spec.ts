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
  ['print("REV", m.revision, sorted(m.get_var_names()))'],
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

/** Click into cell `index`'s editor and run it with Shift+Enter. */
async function runCell(page: Page, index: number): Promise<Locator> {
  const cell = page.locator('.jp-Notebook .jp-Cell').nth(index);
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
  const cells = page.locator('.jp-Notebook .jp-Cell');
  await expect(cells).toHaveCount(CELLS.length, { timeout: 90_000 });

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
  expect(revisionIn(afterEditor)).toBeGreaterThanOrEqual(1);
  expect(afterEditor).toContain("'new_variable'");
  const runLine = afterEditor.split('\n').find((line) => line.startsWith('RUN'));
  expect(runLine).toContain("'new_variable'");
  const revisionAfterEditor = revisionIn(afterEditor);

  // --- AC2.7: the selection is readable from Python ---------------------
  const selection = await cellOutput(await runCell(page, 2), 'SEL');
  expect(selection).toContain("('new_variable',)");

  // --- a Python edit reaches the Editor ---------------------------------
  const notice = widget.getByRole('status');
  const afterPython = await cellOutput(await runCell(page, 3), 'REV');
  expect(revisionIn(afterPython)).toBe(revisionAfterEditor + 1);
  await expect(notice).toHaveText('Updated from Python');
  // Kernel-added variables get a diagram element from the incremental
  // layout, whose label breaks long names across lines (so the label's
  // text content may run the words together).
  await expect(canvas.locator('g.simlin-aux', { hasText: /from\s*python/ })).toBeVisible();

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

  // Kept as visual artifacts of the run (gitignored), not assertions.
  await page.screenshot({ path: path.join(__dirname, '.output', 'journey-notebook.png') });
  await widget.scrollIntoViewIfNeeded();
  await widget.screenshot({ path: path.join(__dirname, '.output', 'journey-widget.png') });

  // --- nothing went wrong along the way ---------------------------------
  await expect(page.locator('.jp-OutputArea-output[data-mime-type="application/vnd.jupyter.error"]')).toHaveCount(0);
  await expect(page.locator('.jp-OutputArea-output[data-mime-type="application/vnd.jupyter.stderr"]')).toHaveCount(0);
  expect(consoleErrors).toEqual([]);
});
