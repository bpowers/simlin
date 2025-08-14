import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './ui-tests',
  outputDir: './test-results',
  
  // Fail the build on CI if you accidentally left test.only in the source code
  forbidOnly: !!process.env.CI,
  
  // Retry on CI only
  retries: process.env.CI ? 2 : 0,
  
  // Opt out of parallel tests on CI
  workers: process.env.CI ? 1 : undefined,
  
  // Reporter to use
  reporter: process.env.CI ? 'github' : 'html',
  
  use: {
    // Base URL for all tests
    baseURL: 'http://localhost:3000',
    
    // Collect trace when retrying the failed test
    trace: 'on-first-retry',
    
    // Screenshot on failure
    screenshot: 'only-on-failure',
  },

  projects: [
    {
      name: 'visual',
      testMatch: /visual\/.+\.spec\.ts$/,
      use: {
        ...devices['Desktop Chrome'],
        // Fixed viewport for consistent visual tests
        viewport: { width: 1280, height: 720 },
        // Disable animations for visual consistency
        launchOptions: {
          args: ['--force-prefers-reduced-motion'],
        },
      },
    },
    {
      name: 'integration',
      testMatch: /integration\/.+\.spec\.ts$/,
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1280, height: 720 },
      },
    },
  ],

  // Run your local dev server before starting the tests
  // For integration tests, we need all services running including Firebase Auth emulator
  webServer: process.env.TEST_MODE === 'integration' ? [
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
  ] : [
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