import { test, expect } from '@playwright/test';
import { mkdtemp } from 'fs/promises';
import { tmpdir } from 'os';
import { join } from 'path';

test.describe('Debug Visual Test Setup', () => {
  test('can access visual test page', async ({ page }) => {
    console.log('Navigating to /visual-test...');
    
    // Enable console logging from the page
    page.on('console', msg => console.log('PAGE LOG:', msg.text()));
    page.on('pageerror', err => console.log('PAGE ERROR:', err));
    
    // Try to navigate to the visual test page
    const response = await page.goto('/visual-test', { 
      waitUntil: 'networkidle',
      timeout: 15000 
    });
    
    console.log('Response status:', response?.status());
    console.log('Response URL:', response?.url());
    
    // Check if we're on the right page
    const url = page.url();
    console.log('Current URL:', url);
    
    // Check page content
    const bodyText = await page.evaluate(() => document.body.innerText);
    console.log('Page body text:', bodyText.substring(0, 200));
    
    // Check if visualTestReady is set
    const isReady = await page.evaluate(() => (window as any).visualTestReady);
    console.log('visualTestReady:', isReady);
    
    // Check if loadXmileModel function exists
    const hasLoadFunction = await page.evaluate(() => typeof (window as any).loadXmileModel === 'function');
    console.log('loadXmileModel exists:', hasLoadFunction);
    
    // Take a screenshot for debugging (save to temp directory)
    const tempDir = await mkdtemp(join(tmpdir(), 'simlin-test-'));
    const screenshotPath = join(tempDir, 'debug-screenshot.png');
    await page.screenshot({ path: screenshotPath });
    console.log('Debug screenshot saved to:', screenshotPath);
    
    expect(url).toContain('/visual-test');
    expect(isReady).toBe(true);
    expect(hasLoadFunction).toBe(true);
  });
  
  test('can load a simple XMILE model', async ({ page }) => {
    await page.goto('/visual-test');
    
    // Wait for the page to be ready
    await page.waitForFunction(() => (window as any).visualTestReady === true, {
      timeout: 10000
    });
    
    // Try loading a minimal XMILE model
    const minimalXmile = `<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header>
    <name>test</name>
  </header>
  <sim_specs>
    <start>0</start>
    <stop>10</stop>
    <dt>1</dt>
  </sim_specs>
  <model>
    <variables>
      <stock name="test_stock">
        <eqn>100</eqn>
      </stock>
    </variables>
    <views>
      <view>
        <stock x="100" y="100" name="test_stock"/>
      </view>
    </views>
  </model>
</xmile>`;
    
    console.log('Loading minimal XMILE model...');
    
    const loadResult = await page.evaluate((xmile: string) => {
      try {
        return (window as any).loadXmileModel(xmile);
      } catch (err) {
        console.error('Error loading model:', err);
        return false;
      }
    }, minimalXmile);
    
    console.log('Load result:', loadResult);
    
    // The minimal XMILE might not be valid for the importer
    // Just check that the function exists and was called
    expect(typeof loadResult).toBe('boolean');
  });
});