# UI Tests - Visual Regression

This directory contains UI tests for the Simlin frontend, with a focus on visual regression testing using Playwright.

## Setup

First, install dependencies:
```bash
yarn install
npx playwright install chromium
```

## Running Tests

### First Time Setup
On the first run, tests will fail because there are no baseline screenshots. Create them:
```bash
yarn test:visual:update
```

### Run all visual tests
```bash
yarn test:visual
```

### Update baseline screenshots
When intentional UI changes are made, update the baseline screenshots:
```bash
yarn test:visual:update
```

### View test results interactively
```bash
yarn test:visual:ui
```

## Test Structure

- `visual/default-projects.spec.ts` - Tests that render each default project and compare against baseline screenshots
- `visual/diagram-elements.spec.ts` - Tests specific diagram elements (stocks, flows, auxiliaries) for visual consistency

## How It Works

1. The tests use a special `/visual-test` route that loads a minimal React component (`VisualTestPage.tsx`)
2. This component exposes `loadXmileModel()` function to load and render XMILE models directly
3. Playwright takes screenshots of the rendered SVG diagrams
4. Screenshots are compared against baseline images stored in `ui-tests/visual/**/*-expected.png`

## Baseline Management

### Understanding Screenshot Files

Playwright creates screenshots with platform-specific suffixes:
- `*-visual-darwin.png` - Actual screenshots on macOS
- `*-visual-linux.png` - Actual screenshots on Linux (CI)
- `*-visual-win32.png` - Actual screenshots on Windows

When you run `yarn test:visual:update`, these become the baseline screenshots for comparison.

### When to Update Baselines

Update baselines when:
- Initial setup (first time running tests)
- Intentional design changes are made to diagram rendering
- New visual elements are added
- Layout algorithms are improved

### How to Update

1. Review the failing tests to ensure changes are intentional
2. Run `yarn test:visual:update` to update all baselines
3. Review the new baseline images before committing
4. Commit the new `*.png` files in the `*-snapshots/` directories

## CI/CD Integration

Visual tests run automatically on:
- Every push to `main`
- Every pull request

Failed tests will upload diff artifacts showing:
- Expected image
- Actual image  
- Diff highlighting the changes

## Troubleshooting

### Tests fail locally but pass in CI
- Ensure you're using the same Chrome version as CI
- Check viewport size is consistent (1280x720)
- Disable system animations

### Flaky tests
- Increase `waitForTimeout` in test if layout needs more time to stabilize
- Use more specific selectors
- Increase `maxDiffPixels` threshold for acceptable variations

### Cannot find visual test page
- Ensure the dev server is running: `yarn start:frontend`
- Check that `/visual-test` route is properly configured in App.tsx
- Verify VisualTestPage.tsx is properly imported