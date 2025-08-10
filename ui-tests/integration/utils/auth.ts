import { Page, expect } from '@playwright/test';

export interface TestUser {
  email: string;
  password: string;
  fullName: string;
  username: string;
}

export function makeTestUser(prefix = 'playwright'): TestUser {
  const ts = Date.now();
  return {
    email: `${prefix}.${ts}@example.com`,
    password: 'TestPassword123!',
    fullName: `Test User ${ts}`,
    username: `testuser-${ts}`,
  };
}

export async function signInWithEmail(page: Page, email: string) {
  await page.goto('/');
  await page.waitForSelector('.simlin-login-outer', { state: 'visible' });
  await page.getByRole('button', { name: 'Sign in with email' }).click();
  await page.getByRole('textbox', { name: /email/i }).fill(email);
  await page.getByRole('button', { name: 'Next' }).click();
}

export async function completeCreateAccount(page: Page, fullName: string, password: string) {
  // Wait for create account form
  await expect(page.getByText('Create account')).toBeVisible();
  // Some MUI forms render name without explicit type; target second input
  const inputs = page.locator('input');
  await inputs.nth(1).fill(fullName);
  // Fill password using input[type=password] for robustness
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole('button', { name: 'Save' }).click();
}

export async function completeNewUserSetup(page: Page, username: string) {
  // Wait for Welcome dialog
  await expect(page.getByText('Welcome!')).toBeVisible();
  await page.getByLabel('Username').fill(username);
  // Agree to terms
  // The checkbox accessible name comes from the full label text; be robust
  const checkbox = page.getByRole('checkbox').first();
  await expect(checkbox).toBeVisible();
  await checkbox.check();
  const submit = page.getByRole('button', { name: 'Submit' });
  await expect(submit).toBeEnabled();
  await submit.click();
}

export async function createAndLoginNewUser(page: Page, user: TestUser) {
  await signInWithEmail(page, user.email);
  await page.waitForTimeout(1500); // let emulator respond
  await completeCreateAccount(page, user.fullName, user.password);
  await completeNewUserSetup(page, user.username);
  // After setup, either Home or the editor could be shown depending on routing; be flexible.
  const home = page.locator('.simlin-home-root');
  const canvas = page.locator('.simlin-canvas');
  const searchbar = page.locator('.simlin-editor-searchbar');
  try {
    await Promise.race([
      home.waitFor({ state: 'visible', timeout: 3000 }),
      canvas.waitFor({ state: 'visible', timeout: 3000 }),
      searchbar.waitFor({ state: 'visible', timeout: 3000 }),
    ]);
  } catch {
    // Don’t fail here; downstream tests navigate to specific routes and will assert visibility there.
  }
}
