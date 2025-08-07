# Simlin Frontend Testing Implementation

## Executive Summary

This document describes the implemented visual regression testing strategy for the Simlin frontend. The solution provides:
- Visual regression testing for SVG diagram rendering using Playwright
- Minimal new dependencies (only Playwright as dev dependency)
- Zero impact on production bundle size
- Test infrastructure isolated from production code
- Automated baseline management for visual comparisons

## Implemented Architecture

### Visual Regression Testing (Playwright)

**Why Playwright:**
- Built-in screenshot comparison with baseline management
- Cross-browser testing capability (currently using Chrome)
- Fast execution with parallel test support
- Native TypeScript support
- Excellent debugging tools and HTML reports
- CI/CD ready with GitHub Actions support

**Implementation:**
The visual regression tests verify that diagram rendering remains consistent across code changes by comparing screenshots against baseline images.

## Project Structure

```
simlin-frontend/
├── ui-tests/                      # All UI tests
│   ├── visual/                    # Visual regression tests
│   │   ├── default-projects.spec.ts    # Tests for default project diagrams
│   │   ├── diagram-elements.spec.ts    # Tests for individual SVG elements
│   │   ├── basic.spec.ts              # Basic functionality tests
│   │   ├── debug.spec.ts              # Debug helper tests
│   │   └── *-snapshots/               # Baseline screenshots (auto-generated)
│   ├── README.md                  # Test documentation
│   └── TROUBLESHOOTING.md        # Debugging guide
├── playwright.config.ts           # Playwright configuration
└── src/app/VisualTestPage.tsx   # Test harness component (dev-only)
```

## Key Implementation Details

### 1. Test Harness

Created a dedicated `VisualTestPage` component that:
- Loads XMILE models directly without authentication
- Renders diagrams using the existing Canvas component
- Exposes `window.loadXmileModel()` for test automation
- Only available in development mode (excluded from production builds)

### 2. Development-Only Route

The `/visual-test` route is conditionally loaded:
```typescript
// Only loaded when NODE_ENV !== 'production'
const VisualTestPage = process.env.NODE_ENV !== 'production' 
  ? React.lazy(() => import('./VisualTestPage'))
  : null;
```

This ensures:
- No test code in production bundles
- Route doesn't exist in production
- Authentication bypass only in development

### 3. Visual Test Implementation

Tests use structural selectors instead of CSS classes (which are dynamically generated):
```typescript
// Find elements by their SVG structure
const stocks = page.locator('svg.simlin-canvas g > rect');
const flows = page.locator('svg.simlin-canvas path');
const auxiliaries = page.locator('svg.simlin-canvas circle');
const labels = page.locator('svg.simlin-canvas text');
```

### 4. Baseline Management

Screenshots are stored with platform-specific suffixes:
- `*-visual-darwin.png` - macOS baselines
- `*-visual-linux.png` - Linux baselines (CI)
- `*-visual-win32.png` - Windows baselines

### 5. Debug Screenshots

Debug screenshots are saved to temp directories to avoid repository clutter:
```typescript
const tempDir = await mkdtemp(join(tmpdir(), 'simlin-test-'));
await page.screenshot({ path: join(tempDir, 'debug.png') });
```

## Test Commands

```bash
# First time setup - create baseline screenshots
yarn test:visual:update

# Run visual regression tests
yarn test:visual

# Update baselines after intentional changes
yarn test:visual:update

# Debug with visible browser
yarn test:debug

# View test report
npx playwright show-report
```

## CI/CD Integration

### GitHub Actions Workflow

```yaml
name: Visual Regression Tests

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  visual-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
      - name: Install dependencies
        run: yarn install
      - name: Build WASM modules
        run: yarn build
      - name: Install Playwright
        run: npx playwright install chromium
      - name: Run visual tests
        run: yarn test:visual
      - name: Upload artifacts on failure
        if: failure()
        uses: actions/upload-artifact@v4
        with:
          name: visual-diffs
          path: test-results/
```

## Configuration Files

### playwright.config.ts
```typescript
export default defineConfig({
  testDir: './ui-tests',
  outputDir: './test-results',
  
  projects: [{
    name: 'visual',
    testMatch: /visual\/.+\.spec\.ts$/,
    use: {
      ...devices['Desktop Chrome'],
      viewport: { width: 1280, height: 720 },
    },
  }],

  webServer: {
    command: 'yarn start:frontend',
    port: 3000,
    reuseExistingServer: !process.env.CI,
  },
});
```

## Test Coverage

### What's Tested

1. **Default Projects** - All 4 default projects render correctly:
   - Population model
   - Logistic growth model
   - Fishbanks model
   - Reliability model

2. **Diagram Elements** - Individual components render properly:
   - Stock elements (rectangles)
   - Flow elements (paths with arrows)
   - Auxiliary variables (circles)
   - Connectors (paths linking elements)
   - Text labels

3. **Layout Stability** - Diagrams maintain consistent layout:
   - On page reload
   - Across different model complexities

## Migration from Proposal

### What Changed

1. **Removed Unit Testing** - Focused solely on visual regression testing as the immediate need
2. **Removed Integration Testing** - Simplified to visual tests only
3. **Used Playwright Instead of Multiple Tools** - Single tool for all visual testing needs
4. **Moved Tests to ui-tests/** - Better naming than generic "tests" directory
5. **Added Development-Only Protection** - Test infrastructure not available in production

### Why These Changes

- **Pragmatic Focus**: Visual regression was the primary need for catching diagram rendering issues
- **Minimal Dependencies**: Only added Playwright, avoiding multiple testing frameworks
- **Security**: Test routes and components completely excluded from production
- **Simplicity**: One tool, one purpose, easy to maintain

## Success Metrics Achieved

✅ **Test Execution Time**: < 10 seconds for full visual suite  
✅ **Zero Production Impact**: Test code completely excluded from production builds  
✅ **Developer Experience**: Simple commands, clear output, easy debugging  
✅ **Baseline Management**: Automatic platform-specific baselines  
✅ **CI/CD Integration**: Ready for GitHub Actions  

## Future Enhancements

When needed, the testing infrastructure can be extended with:

1. **Unit Tests**: Add Vitest for component logic testing
2. **Integration Tests**: Add E2E tests for user workflows
3. **Performance Tests**: Add metrics collection for render times
4. **Accessibility Tests**: Add automated a11y checks

The current implementation provides a solid foundation that can evolve with the codebase while immediately catching visual regressions in diagram rendering.