import { test, expect } from '@playwright/test';
import { createAndLoginNewUser, makeTestUser } from './utils/auth';

type KatexChar = { ch: string; left: number; top: number; width: number; height: number };

async function getEditorText(page: any): Promise<string> {
  const editor = page.locator('.simlin-variabledetails-eqneditor');
  await expect(editor).toBeVisible();
  return (await editor.textContent()) || '';
}

async function getCaretInfo(page: any): Promise<{ caret: number; ascii: string; left: string; right: string; context: string }> {
  return await page.evaluate(() => {
    const editor = document.querySelector('.simlin-variabledetails-eqneditor');
    if (!editor) return { caret: -1, ascii: '', left: '', right: '', context: '' };
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0) return { caret: -1, ascii: '', left: '', right: '', context: '' };
    const range = sel.getRangeAt(0);
    const pre = range.cloneRange();
    pre.selectNodeContents(editor);
    pre.setEnd(range.startContainer, range.startOffset);
    const caret = pre.toString().length;
    const ascii = (editor.textContent || '');
    const left = ascii.slice(Math.max(0, caret - 12), caret);
    const right = ascii.slice(caret, Math.min(ascii.length, caret + 12));
    const context = `${left}|${right}`;
    return { caret, ascii, left, right, context };
  });
}

async function returnToPreview(page: any) {
  await page.keyboard.press('Escape');
  await expect(page.locator('.simlin-variabledetails-eqnpreview')).toBeVisible();
}

async function getKatexChars(page: any): Promise<KatexChar[]> {
  const chars: KatexChar[] = await page.evaluate(() => {
    function collectChars(root: Element): { ch: string; left: number; top: number; width: number; height: number }[] {
      const out: { ch: string; left: number; top: number; width: number; height: number }[] = [];
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
      let node: Node | null;
      while ((node = walker.nextNode())) {
        const tn = node as Text;
        const text = tn.nodeValue || '';
        for (let i = 0; i < text.length; i++) {
          const ch = text[i];
          const rng = document.createRange();
          rng.setStart(tn, i);
          rng.setEnd(tn, i + 1);
          const rect = rng.getBoundingClientRect();
          if (rect && rect.width > 0 && rect.height > 0) {
            out.push({ ch, left: rect.left, top: rect.top, width: rect.width, height: rect.height });
          }
        }
      }
      return out;
    }
    const container = document.querySelector('.simlin-variabledetails-eqnpreview .katex') as Element | null;
    if (!container) return [];
    return collectChars(container);
  });
  return chars;
}

function findFirstCharIndex(chars: KatexChar[], candidates: string[]): number {
  const set = new Set(candidates);
  return chars.findIndex((c) => set.has(c.ch));
}

function mapKatexCharToAsciiChar(kchar: string): string {
  if (kchar === '·' || kchar === '×' || kchar === '⋅') return '*';
  if (kchar === '−') return '-';
  return kchar;
}

test.describe('Caret mapping by clicking specific KaTeX glyphs', () => {
  test('clicking specific rendered glyphs places caret right after matching ASCII char', async ({ page }) => {
    const user = makeTestUser('caret-multi2');
    await createAndLoginNewUser(page, user);

    await page.goto(`/${user.username}/logistic-growth`);
    await page.waitForSelector('.simlin-canvas', { state: 'visible' });

    // Open VariableDetails via search
    const searchBar = page.locator('.simlin-editor-searchbar');
    await expect(searchBar).toBeVisible();
    const searchInput = searchBar.locator('input');
    await searchInput.click();
    await searchInput.fill('fractional');
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Enter');

    const preview = page.locator('.simlin-variabledetails-eqnpreview');
    const editor = page.locator('.simlin-variabledetails-eqneditor');
    await expect(preview.or(editor)).toBeVisible();
    if (await editor.isVisible()) {
      await returnToPreview(page);
    }

    // Open editor once to capture the ASCII equation for mapping
    // Click the preview near the start
    const pvBox = await preview.boundingBox();
    if (!pvBox) throw new Error('Missing preview bounding box');
    await page.mouse.click(pvBox.x + 10, pvBox.y + pvBox.height / 2);
    const ascii = await getEditorText(page);
    await returnToPreview(page);

    // Build list of glyphs to test in order they appear visually
    const chars = await getKatexChars(page);
    expect(chars.length).toBeGreaterThan(0);

    // Choose a few robust glyphs that should exist: '*', '(', '1', '-', ')'
    const glyphSpecs: { candidates: string[]; describe: string }[] = [
      { candidates: ['m', 'M'], describe: 'letter-m' },
      { candidates: ['⋅', '·', '×', '*'], describe: 'multiplication' },
      { candidates: ['('], describe: 'open-paren' },
      { candidates: ['1'], describe: 'one' },
      { candidates: ['−', '-'], describe: 'minus' },
      { candidates: [')'], describe: 'close-paren' },
    ];

    // Running index in ASCII for sequential searches
    let searchFrom = 0;
    for (const spec of glyphSpecs) {
      const kidx = findFirstCharIndex(chars, spec.candidates);
      expect(kidx, `KaTeX glyph not found for ${spec.describe}`).toBeGreaterThanOrEqual(0);
      const g = chars[kidx];
      const centerX = g.left + g.width / 2;
      const centerY = g.top + g.height / 2;
      await page.mouse.click(centerX, centerY);
      await expect(editor).toBeVisible();

      const info = await getCaretInfo(page);
      const targetAsciiChar = mapKatexCharToAsciiChar(g.ch);
      const expectedPos = ascii.indexOf(targetAsciiChar, searchFrom);
      expect(expectedPos, `ASCII char '${targetAsciiChar}' not found after ${searchFrom} for ${spec.describe}`).toBeGreaterThanOrEqual(0);

      // Expected caret is immediately after that char
      const expectedCaret = expectedPos + 1;
      console.log(`Clicked ${spec.describe} '${g.ch}' -> caret ${info.caret}, expected ${expectedCaret}, context: ${info.context}`);
      expect(info.caret, `${spec.describe} caret mismatch`).toBe(expectedCaret);

      // Advance searchFrom to just after this occurrence to avoid matching earlier duplicates
      searchFrom = expectedCaret;
      await returnToPreview(page);
    }
  });
});
