import { test, expect } from '@playwright/test';
import { readFile } from 'fs/promises';
import { join } from 'path';

async function loadModel(page: any, modelPath: string) {
  const xmileContent = await readFile(modelPath, 'utf-8');
  
  await page.goto('/visual-test');
  await page.waitForFunction(() => (window as any).visualTestReady === true);
  
  const success = await page.evaluate((xmile: string) => {
    return (window as any).loadXmileModel(xmile);
  }, xmileContent);
  
  if (!success) {
    throw new Error(`Failed to load model from ${modelPath}`);
  }
  
  await page.waitForSelector('svg.simlin-canvas', { timeout: 10000 });
  await page.waitForTimeout(500);
}

test.describe('Diagram Element Visual Tests', () => {
  test.beforeEach(async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 720 });
  });

  test('stock elements render with correct appearance', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/population/model.xmile');
    await loadModel(page, modelPath);
    
    // Stocks are rendered as rect elements within g elements
    // The styled-components will add dynamic class names
    const stocks = page.locator('svg.simlin-canvas g > rect').first();
    
    // Verify at least one stock exists
    await expect(stocks).toBeVisible();
    
    // Take a focused screenshot of the stock element area
    await expect(stocks).toHaveScreenshot('stock-element.png', {
      maxDiffPixels: 50,
      threshold: 0.2,
    });
  });

  test('flow elements render with paths', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/population/model.xmile');
    await loadModel(page, modelPath);
    
    // Flows are rendered as path elements
    // Look for paths that are likely flows (not connectors)
    const flows = page.locator('svg.simlin-canvas path[d*="M"]');
    const count = await flows.count();
    
    // Population model should have some paths (flows and connectors)
    expect(count).toBeGreaterThan(0);
    
    // Take screenshot of the whole canvas since flows connect elements
    const canvas = page.locator('svg.simlin-canvas');
    await expect(canvas).toHaveScreenshot('flows-diagram.png', {
      maxDiffPixels: 100,
      threshold: 0.2,
    });
  });

  test('auxiliary variables render as circles', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/population/model.xmile');
    await loadModel(page, modelPath);
    
    // Auxiliaries are rendered as circle elements
    const auxiliaries = page.locator('svg.simlin-canvas circle').first();
    
    // Check if we have at least one circle (auxiliary)
    const circleCount = await page.locator('svg.simlin-canvas circle').count();
    expect(circleCount).toBeGreaterThan(0);
    
    // Take screenshot of auxiliary element if it exists
    if (circleCount > 0) {
      await expect(auxiliaries).toHaveScreenshot('aux-element.png', {
        maxDiffPixels: 50,
        threshold: 0.2,
      });
    }
  });

  test('canvas renders complete diagram', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/population/model.xmile');
    await loadModel(page, modelPath);
    
    const canvas = page.locator('svg.simlin-canvas');
    
    // Verify the SVG has content
    const hasContent = await page.evaluate(() => {
      const svg = document.querySelector('svg.simlin-canvas');
      if (!svg) return false;
      // Check for g elements which contain the diagram elements
      const gElements = svg.querySelectorAll('g');
      return gElements.length > 0;
    });
    
    expect(hasContent).toBe(true);
    
    // Take a screenshot of the complete diagram
    await expect(canvas).toHaveScreenshot('population-complete.png', {
      maxDiffPixels: 100,
      threshold: 0.2,
    });
  });

  test('text labels are rendered', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/population/model.xmile');
    await loadModel(page, modelPath);
    
    // Wait a bit for text to render
    await page.waitForTimeout(1000);
    
    // Look for text elements in the SVG
    const textElements = page.locator('svg.simlin-canvas text');
    const count = await textElements.count();
    
    // Should have text labels for variables
    expect(count).toBeGreaterThan(0);
    
    // Check if specific text exists
    const hasPopulationText = await page.evaluate(() => {
      const texts = document.querySelectorAll('svg.simlin-canvas text');
      for (const text of texts) {
        if (text.textContent?.toLowerCase().includes('population')) {
          return true;
        }
      }
      return false;
    });
    
    expect(hasPopulationText).toBe(true);
  });

  test('complex model renders all elements', async ({ page }) => {
    const modelPath = join(process.cwd(), 'default_projects/fishbanks/model.xmile');
    await loadModel(page, modelPath);
    
    const diagram = page.locator('svg.simlin-canvas');
    
    // For complex models, just ensure overall layout is stable
    await expect(diagram).toHaveScreenshot('fishbanks-full-diagram.png', {
      maxDiffPixels: 200,
      threshold: 0.2,
      fullPage: false,
    });
    
    // Verify the diagram has complexity - check for various element types
    const rectCount = await page.locator('svg.simlin-canvas rect').count();
    const circleCount = await page.locator('svg.simlin-canvas circle').count();
    const pathCount = await page.locator('svg.simlin-canvas path').count();
    const textCount = await page.locator('svg.simlin-canvas text').count();
    
    // Fishbanks is a complex model, should have multiple elements of each type
    expect(rectCount).toBeGreaterThan(0); // stocks
    expect(circleCount).toBeGreaterThan(0); // auxiliaries
    expect(pathCount).toBeGreaterThan(0); // flows and connectors
    expect(textCount).toBeGreaterThan(0); // labels
  });
});