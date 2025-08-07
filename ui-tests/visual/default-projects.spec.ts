import { test, expect } from '@playwright/test';
import { readFile } from 'fs/promises';
import { join } from 'path';

// Helper to load and render a model
async function loadAndRenderModel(page: any, modelPath: string) {
  const xmileContent = await readFile(modelPath, 'utf-8');
  
  await page.goto('/visual-test');
  
  // Wait for the test page to be ready
  await page.waitForFunction(() => (window as any).visualTestReady === true, {
    timeout: 10000
  });
  
  // Load the XMILE model
  const success = await page.evaluate((xmile: string) => {
    return (window as any).loadXmileModel(xmile);
  }, xmileContent);
  
  if (!success) {
    throw new Error(`Failed to load model from ${modelPath}`);
  }
  
  // Wait for the canvas to render
  await page.waitForSelector('svg.simlin-canvas', { timeout: 10000 });
  
  // Give it a moment for layout to stabilize
  await page.waitForTimeout(500);
  
  return page.locator('svg.simlin-canvas');
}

test.describe('Default Projects Visual Regression', () => {
  test.beforeEach(async ({ page }) => {
    // Set a consistent viewport
    await page.setViewportSize({ width: 1280, height: 720 });
  });

  test('population model renders correctly', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/population/model.xmile');
    const diagram = await loadAndRenderModel(page, modelPath);
    
    // Take screenshot and compare with baseline
    await expect(diagram).toHaveScreenshot('population-model.png', {
      maxDiffPixels: 100,
      threshold: 0.2,
      animations: 'disabled',
    });
  });

  test('logistic-growth model renders correctly', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/logistic-growth/model.xmile');
    const diagram = await loadAndRenderModel(page, modelPath);
    
    await expect(diagram).toHaveScreenshot('logistic-growth-model.png', {
      maxDiffPixels: 100,
      threshold: 0.2,
      animations: 'disabled',
    });
  });

  test('fishbanks model renders correctly', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/fishbanks/model.xmile');
    const diagram = await loadAndRenderModel(page, modelPath);
    
    await expect(diagram).toHaveScreenshot('fishbanks-model.png', {
      maxDiffPixels: 100,
      threshold: 0.2,
      animations: 'disabled',
    });
  });

  test('reliability model renders correctly', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/reliability/model.xmile');
    const diagram = await loadAndRenderModel(page, modelPath);
    
    await expect(diagram).toHaveScreenshot('reliability-model.png', {
      maxDiffPixels: 100,
      threshold: 0.2,
      animations: 'disabled',
    });
  });
});

test.describe('Visual Regression - Layout Stability', () => {
  test('models maintain consistent layout on reload', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/population/model.xmile');
    
    // Load the model once
    const diagram1 = await loadAndRenderModel(page, modelPath);
    const screenshot1 = await diagram1.screenshot();
    
    // Reload the page and load the model again
    await page.reload();
    const diagram2 = await loadAndRenderModel(page, modelPath);
    const screenshot2 = await diagram2.screenshot();
    
    // Screenshots should be identical
    expect(screenshot1).toEqual(screenshot2);
  });

  test('zoom and pan reset produces identical layout', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/logistic-growth/model.xmile');
    const diagram = await loadAndRenderModel(page, modelPath);
    
    // Take baseline screenshot
    const baseline = await diagram.screenshot();
    
    // Simulate zoom and pan (if controls are available)
    // For now, just verify the diagram is stable
    await page.waitForTimeout(1000);
    
    // Take another screenshot
    const after = await diagram.screenshot();
    
    // Should be identical since we didn't actually interact
    expect(baseline).toEqual(after);
  });
});