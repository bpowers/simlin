import { test, expect } from '@playwright/test';
import { readFile, mkdtemp } from 'fs/promises';
import { join } from 'path';
import { tmpdir } from 'os';

test.describe('Basic Visual Test', () => {
  test('visual test page loads', async ({ page }) => {
    // Enable console logging
    page.on('console', msg => {
      if (msg.type() === 'error') {
        console.log('Browser console error:', msg.text());
      }
    });
    
    await page.goto('/visual-test');
    
    // Check if we reached the right page
    const url = page.url();
    expect(url).toContain('/visual-test');
    
    // Wait for the visual test page to be ready
    await page.waitForFunction(
      () => (window as any).visualTestReady === true,
      { timeout: 5000 }
    ).catch(err => {
      console.error('visualTestReady not found:', err);
      throw err;
    });
    
    // Verify the loadXmileModel function exists
    const hasFunction = await page.evaluate(() => {
      return typeof (window as any).loadXmileModel === 'function';
    });
    expect(hasFunction).toBe(true);
  });

  test('can render population model', async ({ page }) => {
    await page.goto('/visual-test');
    
    // Wait for ready
    await page.waitForFunction(() => (window as any).visualTestReady === true);
    
    // Load the population model
    const modelPath = join(process.cwd(), 'default_projects/population/model.xmile');
    const xmileContent = await readFile(modelPath, 'utf-8');
    
    const loadSuccess = await page.evaluate((xmile: string) => {
      return (window as any).loadXmileModel(xmile);
    }, xmileContent);
    
    expect(loadSuccess).toBe(true);
    
    // Wait for the SVG canvas to appear
    const canvas = await page.waitForSelector('svg.simlin-canvas', { 
      state: 'visible',
      timeout: 5000 
    });
    
    expect(canvas).toBeTruthy();
    
    // Verify the canvas has some content
    const hasContent = await page.evaluate(() => {
      const svg = document.querySelector('svg.simlin-canvas');
      if (!svg) return false;
      // Check if there are child elements (should have at least defs and g)
      return svg.children.length > 0;
    });
    
    expect(hasContent).toBe(true);
    
    // Take a simple screenshot (save to temp directory for debugging)
    const tempDir = await mkdtemp(join(tmpdir(), 'simlin-test-'));
    const screenshotPath = join(tempDir, 'population-test.png');
    await canvas.screenshot({ path: screenshotPath });
    console.log('Population test screenshot saved to:', screenshotPath);
  });
});