import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './ui-tests/integration',
  outputDir: './test-results',

  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: process.env.CI ? 'github' : 'html',

  use: {
    baseURL: 'http://localhost:3000',
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    ...devices['Desktop Chrome'],
    channel: 'chromium',
    viewport: { width: 1280, height: 720 },
  },

  webServer: [
    {
      command: 'yarn start:firestore',
      port: 8092,
      reuseExistingServer: !process.env.CI,
      timeout: 120 * 1000,
    },
    {
      command: 'cd src/app && yarn firebase emulators:start --only auth',
      port: 9099,
      reuseExistingServer: !process.env.CI,
      timeout: 120 * 1000,
    },
    {
      command: './scripts/start-backend-for-tests.sh',
      port: 3030,
      reuseExistingServer: !process.env.CI,
      timeout: 120 * 1000,
    },
    {
      command: 'yarn start:frontend',
      port: 3000,
      reuseExistingServer: !process.env.CI,
      timeout: 120 * 1000,
    },
  ],
});