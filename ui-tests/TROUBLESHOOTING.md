# Visual Test Troubleshooting Guide

## Setup Steps

1. **Install dependencies**:
```bash
yarn install
npx playwright install chromium
```

2. **Build the application** (required for WASM modules):
```bash
yarn build
```

3. **Run basic test to verify setup**:
```bash
yarn test:basic
```

## Common Issues

### Tests timing out

If tests are timing out, check:

1. **Dev server is running**: The Playwright config should auto-start it, but you can manually run:
```bash
yarn start:frontend
```

2. **Visual test page is accessible**: Visit http://localhost:3000/visual-test in your browser
   - Should show "Waiting for model..." text
   - Open browser console and check for `window.visualTestReady` (should be `true`)

3. **Check for build issues**: Ensure WASM modules are built:
```bash
cd src/engine && ./build.sh && cd ../..
cd src/importer && ./build.sh && cd ../..
```

### Canvas not rendering

- The Canvas uses class name `simlin-canvas` not `diagram-canvas`
- Check browser console for errors when loading models
- Verify the model XMILE is valid

### Debugging tests

Run debug test with browser visible:
```bash
yarn test:debug
```

This will:
- Open a browser window
- Show console logs
- Take debug screenshots

### Step-by-step verification

1. **Test the route works**:
```bash
# Start dev server
yarn start:frontend

# In another terminal, check the route
curl http://localhost:3000/visual-test
# Should return HTML, not redirect to login
```

2. **Test model loading manually**:
   - Open http://localhost:3000/visual-test in browser
   - Open browser console
   - Run: `window.loadXmileModel('<xmile>...</xmile>')`
   - Should return `true` if successful

3. **Run minimal test**:
```bash
yarn test:basic
```

## Test Order

When debugging, run tests in this order:

1. `yarn test:debug` - Basic connectivity and setup verification
2. `yarn test:basic` - Simple model loading test
3. `yarn test:visual` - Full visual regression suite

## Logs and Artifacts

- Screenshots on failure: `test-results/`
- Debug screenshots: Saved to temp directory (path printed in console output)
- Playwright report: `playwright-report/`

View the HTML report after test run:
```bash
npx playwright show-report
```