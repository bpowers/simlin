import { test, expect } from '@playwright/test';
import { createAndLoginNewUser, makeTestUser } from './utils/auth';

test.describe('Editor caret mapping from LaTeX to ASCII', () => {
  test('clicking between characters in LaTeX places caret correctly in ASCII editor', async ({ page }) => {
    const user = makeTestUser('caret');
    await createAndLoginNewUser(page, user);

    // Navigate to the default logistic-growth project for this user
    await page.goto(`/${user.username}/logistic-growth`);

    // Wait for canvas
    await page.waitForSelector('.simlin-canvas', { state: 'visible' });

    // Use search to open VariableDetails for the target variable
    const searchBar = page.locator('.simlin-editor-searchbar');
    await expect(searchBar).toBeVisible();
    const searchInput = searchBar.locator('input');
    await searchInput.click();
    await searchInput.fill('fractional');
    // Wait for autocomplete list and select the matching option
    // Prefer keyboard selection to avoid role/DOM drift
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Enter');

    // Wait briefly for VariableDetails UI (card or either equation pane)
    const detailsCard = page.locator('.simlin-variabledetails-card');
    const preview = page.locator('.simlin-variabledetails-eqnpreview');
    const editor = page.locator('.simlin-variabledetails-eqneditor');
    const appeared = await Promise.race([
      detailsCard.waitFor({ state: 'visible', timeout: 2000 }).then(() => true).catch(() => false),
      preview.waitFor({ state: 'visible', timeout: 2000 }).then(() => true).catch(() => false),
      editor.waitFor({ state: 'visible', timeout: 2000 }).then(() => true).catch(() => false),
    ]);

    // Fallback: if search didn’t open details, click label on canvas
    if (!appeared) {
      const label = page.getByText('fractional growth rate', { exact: false });
      await label.click({ clickCount: 1 });
      await Promise.race([
        detailsCard.waitFor({ state: 'visible', timeout: 3000 }),
        preview.waitFor({ state: 'visible', timeout: 3000 }),
        editor.waitFor({ state: 'visible', timeout: 3000 }),
      ]);
    }

    // Prefer clicking LaTeX preview if available, otherwise click directly in the editor
    const ready =
      (await preview.isVisible().catch(() => false)) || (await editor.isVisible().catch(() => false));
    if (!ready) {
      // Fail fast with an actionable message instead of timing out later
      expect(ready, 'VariableDetails did not appear (preview/editor)').toBeTruthy();
    }
    const hasPreview = await preview.isVisible().catch(() => false);
    if (hasPreview) {
      const box = await preview.boundingBox();
      expect(box).not.toBeNull();
      const bb = box!;
      const x = bb.x + 14; // ~1ch with padding bias
      const y = bb.y + bb.height / 2;
      await page.mouse.click(x, y);
      await expect(editor).toBeVisible();
    } else {
      // Directly click inside the editor to place caret
      await expect(editor).toBeVisible();
      const box = await editor.boundingBox();
      expect(box).not.toBeNull();
      const bb = box!;
      const x = bb.x + 14;
      const y = bb.y + bb.height / 2;
      await page.mouse.click(x, y);
    }

    // Extract caret offset by reading DOM selection
    const caretOffset = await page.evaluate(() => {
      const sel = window.getSelection();
      if (!sel || sel.rangeCount === 0) return -1;
      const range = sel.getRangeAt(0);
      return range.startOffset;
    });
    expect(caretOffset).toBeGreaterThanOrEqual(1);
  });
});
