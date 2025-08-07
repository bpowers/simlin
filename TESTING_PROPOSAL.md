# Simlin Frontend Testing Strategy

## Executive Summary

This document proposes a comprehensive testing strategy for the Simlin frontend that provides:
- Unit testing for React components
- Visual regression testing for SVG diagram rendering
- Integration testing for critical user workflows
- Minimal new dependencies with zero impact on production bundle size
- Maintainable test infrastructure that evolves with the codebase

## Testing Architecture

### 1. Unit Testing (Vitest + React Testing Library)

**Why Vitest:**
- Fast execution with native ESM support
- TypeScript-first with minimal configuration
- Compatible with existing RSBuild setup
- Excellent watch mode for development
- Built-in coverage reporting
- Jest-compatible API for easy migration

**Implementation:**
```typescript
// Example: src/diagram/drawing/__tests__/Stock.test.tsx
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { Stock, stockContains, stockBounds } from '../Stock';

describe('Stock Component', () => {
  it('renders stock element with correct dimensions', () => {
    const element = new StockViewElement({
      name: 'Population',
      cx: 100,
      cy: 100,
      // ... other props
    });
    
    render(<Stock element={element} isSelected={false} />);
    const stockRect = screen.getByRole('graphics-symbol');
    expect(stockRect).toHaveAttribute('width', '45');
    expect(stockRect).toHaveAttribute('height', '35');
  });

  it('calculates bounds correctly', () => {
    const bounds = stockBounds(element);
    expect(bounds).toEqual({
      top: 82.5,
      left: 77.5,
      right: 122.5,
      bottom: 117.5
    });
  });
});
```

### 2. Visual Regression Testing (Playwright)

**Why Playwright:**
- Built-in screenshot comparison
- Cross-browser testing capability
- Can test both component isolation and full diagrams
- Integrates with CI/CD pipelines
- Supports viewport testing for responsive design

**Implementation:**
```typescript
// Example: tests/visual/diagrams.spec.ts
import { test, expect } from '@playwright/test';
import { readFile } from 'fs/promises';

test.describe('Diagram Visual Regression', () => {
  test('renders population model correctly', async ({ page }) => {
    // Load a known XMILE model
    const xmile = await readFile('test/models/population.xmile', 'utf-8');
    
    await page.goto('/test-harness');
    await page.evaluate((model) => {
      window.loadModel(model);
    }, xmile);
    
    // Wait for rendering to complete
    await page.waitForSelector('svg.diagram-canvas');
    
    // Take screenshot of the SVG diagram
    const diagram = await page.locator('svg.diagram-canvas');
    await expect(diagram).toHaveScreenshot('population-model.png', {
      maxDiffPixels: 100,
      threshold: 0.2
    });
  });

  test('maintains layout after equation changes', async ({ page }) => {
    // Test that diagram layout doesn't break when equations are modified
    await page.goto('/editor');
    await page.click('[data-testid="variable-population"]');
    await page.fill('[data-testid="equation-input"]', 'births - deaths');
    
    const diagram = await page.locator('svg.diagram-canvas');
    await expect(diagram).toHaveScreenshot('population-after-edit.png');
  });
});
```

### 3. Integration Testing (Playwright E2E)

**Test Categories:**

```typescript
// Example: tests/e2e/workflows.spec.ts
test.describe('Model Creation Workflow', () => {
  test('create new model with stock and flow', async ({ page }) => {
    await page.goto('/');
    await page.click('text=New Model');
    
    // Add a stock
    await page.click('[data-tool="stock"]');
    await page.click('svg.canvas', { position: { x: 200, y: 200 } });
    await page.fill('[data-testid="name-input"]', 'Population');
    
    // Add a flow
    await page.click('[data-tool="flow"]');
    await page.dragAndDrop(
      '[data-element="Population"]',
      'svg.canvas',
      { targetPosition: { x: 400, y: 200 } }
    );
    
    // Verify simulation runs
    await page.click('text=Run');
    await expect(page.locator('[data-testid="results-chart"]')).toBeVisible();
  });
});
```

## Test Structure

```
simlin-frontend/
├── src/
│   ├── diagram/
│   │   ├── __tests__/           # Unit tests
│   │   │   ├── Editor.test.tsx
│   │   │   └── utils.test.ts
│   │   └── drawing/
│   │       └── __tests__/
│   │           ├── Stock.test.tsx
│   │           ├── Flow.test.tsx
│   │           └── Canvas.test.tsx
│   ├── app/
│   │   └── __tests__/
│   │       ├── App.test.tsx
│   │       └── Project.test.ts
│   └── core/
│       └── __tests__/
│           ├── canonicalize.test.ts
│           └── datamodel.test.ts
├── tests/
│   ├── visual/                   # Visual regression tests
│   │   ├── diagrams.spec.ts
│   │   ├── components.spec.ts
│   │   └── __screenshots__/      # Baseline images
│   ├── e2e/                      # Integration tests
│   │   ├── workflows.spec.ts
│   │   ├── import-export.spec.ts
│   │   └── simulation.spec.ts
│   └── fixtures/                 # Test data
│       ├── models/
│       └── mock-data/
├── test-utils/                   # Shared test utilities
│   ├── setup.ts
│   ├── render.tsx                # Custom render with providers
│   └── mock-engine.ts
├── vitest.config.ts
└── playwright.config.ts
```

## Configuration

### Vitest Configuration
```typescript
// vitest.config.ts
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    setupFiles: './test-utils/setup.ts',
    coverage: {
      reporter: ['text', 'json', 'html'],
      exclude: [
        'node_modules/',
        'test-utils/',
        '*.config.ts',
        '**/lib/**',
        '**/lib.browser/**',
        '**/core/**'  // WASM files
      ]
    },
    alias: {
      '@system-dynamics/core': path.resolve(__dirname, './src/core'),
      '@system-dynamics/diagram': path.resolve(__dirname, './src/diagram'),
      '@system-dynamics/engine': path.resolve(__dirname, './src/engine'),
    }
  }
});
```

### Playwright Configuration
```typescript
// playwright.config.ts
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  outputDir: './test-results',
  
  use: {
    baseURL: 'http://localhost:3000',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },

  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'firefox',
      use: { ...devices['Desktop Firefox'] },
    },
    {
      name: 'visual',
      testMatch: /visual\/.+\.spec\.ts$/,
      use: {
        ...devices['Desktop Chrome'],
        // Consistent viewport for visual tests
        viewport: { width: 1280, height: 720 },
      },
    },
  ],

  webServer: {
    command: 'yarn start:frontend',
    port: 3000,
    reuseExistingServer: !process.env.CI,
  },
});
```

## Package Updates

### Root package.json
```json
{
  "scripts": {
    "test": "vitest",
    "test:ui": "vitest --ui",
    "test:coverage": "vitest run --coverage",
    "test:visual": "playwright test --project=visual",
    "test:visual:update": "playwright test --project=visual --update-snapshots",
    "test:e2e": "playwright test --project=chromium",
    "test:all": "yarn test:coverage && yarn test:e2e"
  },
  "devDependencies": {
    "@playwright/test": "^1.48.0",
    "@testing-library/react": "^16.1.0",
    "@testing-library/user-event": "^14.5.0",
    "@vitejs/plugin-react": "^4.3.0",
    "jsdom": "^25.0.0",
    "vitest": "^2.1.0",
    "@vitest/ui": "^2.1.0"
  }
}
```

## Testing Patterns

### 1. Component Testing Pattern
```typescript
// test-utils/render.tsx
import { ReactElement } from 'react';
import { render, RenderOptions } from '@testing-library/react';
import { ThemeProvider } from '@mui/material/styles';
import { theme } from '../src/app/theme';

const AllTheProviders = ({ children }: { children: React.ReactNode }) => {
  return (
    <ThemeProvider theme={theme}>
      {children}
    </ThemeProvider>
  );
};

const customRender = (
  ui: ReactElement,
  options?: Omit<RenderOptions, 'wrapper'>
) => render(ui, { wrapper: AllTheProviders, ...options });

export * from '@testing-library/react';
export { customRender as render };
```

### 2. Mock Engine Pattern
```typescript
// test-utils/mock-engine.ts
import { vi } from 'vitest';

export const mockEngine = {
  simulate: vi.fn().mockResolvedValue({
    series: new Map([
      ['time', [0, 1, 2, 3, 4]],
      ['population', [100, 110, 121, 133, 146]]
    ])
  }),
  compile: vi.fn().mockResolvedValue({ success: true }),
  checkUnits: vi.fn().mockResolvedValue({ errors: [] })
};

vi.mock('@system-dynamics/engine', () => ({
  open: vi.fn().mockResolvedValue(mockEngine),
  errorCodeDescription: vi.fn()
}));
```

### 3. SVG Testing Pattern
```typescript
// Helper for testing SVG elements
export function getSvgElement(container: HTMLElement, selector: string) {
  return container.querySelector(selector) as SVGElement | null;
}

export function getTransform(element: SVGElement): { x: number; y: number } {
  const transform = element.getAttribute('transform');
  const match = transform?.match(/translate\(([^,]+),([^)]+)\)/);
  if (match) {
    return {
      x: parseFloat(match[1]),
      y: parseFloat(match[2])
    };
  }
  return { x: 0, y: 0 };
}
```

## CI/CD Integration

### GitHub Actions Workflow
```yaml
# .github/workflows/test.yml
name: Tests

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  unit-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '18'
      - run: yarn install
      - run: yarn test:coverage
      - uses: codecov/codecov-action@v4

  visual-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
      - run: yarn install
      - run: npx playwright install chromium
      - run: yarn build
      - run: yarn test:visual
      - uses: actions/upload-artifact@v4
        if: failure()
        with:
          name: visual-diff-report
          path: test-results/

  e2e-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
      - run: yarn install
      - run: npx playwright install
      - run: yarn build
      - run: yarn test:e2e
```

## Migration Path

### Phase 1: Setup Infrastructure (Week 1)
1. Add testing dependencies to package.json files
2. Create configuration files (vitest.config.ts, playwright.config.ts)
3. Set up test utilities and helpers
4. Create initial smoke tests to verify setup

### Phase 2: Unit Tests (Weeks 2-3)
1. Start with pure utility functions (canonicalize, datamodel helpers)
2. Test isolated React components (Stock, Flow, Aux)
3. Test composed components with mocked dependencies
4. Achieve 60% code coverage target

### Phase 3: Visual Tests (Week 4)
1. Create test harness for rendering diagrams
2. Establish baseline screenshots for key models
3. Add visual tests for component states (selected, hover, error)
4. Document screenshot update process

### Phase 4: Integration Tests (Week 5)
1. Identify critical user workflows
2. Implement E2E tests for model creation
3. Test import/export functionality
4. Test simulation execution

### Phase 5: CI Integration (Week 6)
1. Set up GitHub Actions workflows
2. Configure test reporting
3. Establish code coverage requirements
4. Document testing guidelines

## Maintenance Guidelines

### Test Naming Conventions
```typescript
// Unit tests: describe what the component/function does
describe('stockBounds', () => {
  it('returns correct bounds for stock element', () => {});
  it('handles negative coordinates', () => {});
});

// Visual tests: describe the visual state being tested
test('stock element with warning state', async () => {});
test('complex diagram with 50+ elements', async () => {});

// E2E tests: describe the user journey
test('user can create and simulate a basic SIR model', async () => {});
```

### When to Update Visual Baselines
- Intentional UI changes
- New browser versions (quarterly)
- Component library updates
- Never update baselines to "fix" failing tests without investigation

### Test Data Management
- Store test models in `tests/fixtures/models/`
- Use factory functions for creating test data
- Avoid hardcoding values in tests
- Share common test data across test files

## Performance Considerations

### Test Execution Time Targets
- Unit tests: < 10 seconds for full suite
- Visual tests: < 30 seconds for critical paths
- E2E tests: < 2 minutes for smoke tests
- Full test suite: < 5 minutes

### Optimization Strategies
- Run tests in parallel where possible
- Use test.only during development
- Mock heavy dependencies (WASM engine)
- Cache browser installations
- Use snapshot testing judiciously

## Success Metrics

- **Code Coverage**: 70% for new code, 60% overall
- **Test Execution Time**: < 5 minutes for PR checks
- **Flaky Test Rate**: < 1% of test runs
- **Visual Regression Detection**: 100% of unintended changes caught
- **Developer Adoption**: All new features include tests

## Conclusion

This testing strategy provides comprehensive coverage while maintaining pragmatic constraints:
- Minimal new dependencies (5 dev dependencies)
- Zero impact on production bundle
- Progressive implementation path
- Clear maintenance guidelines

The combination of Vitest for unit testing, React Testing Library for component testing, and Playwright for visual and E2E testing provides a robust foundation that can evolve with the codebase while catching regressions early in the development cycle.