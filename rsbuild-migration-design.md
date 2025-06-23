# Rsbuild Migration Design Document

## Executive Summary

This document outlines a detailed plan for migrating the Simlin project from Webpack + Docusaurus to Rspack/Rsbuild + Rspress. The migration will be implemented in a parallel setup to allow thorough testing before removing the existing Webpack configuration.

## Current State Analysis

### Frontend Build (src/app)

**Current Stack:**
- Webpack 5 with complex custom configuration
- TypeScript compilation via Babel + ForkTsCheckerWebpackPlugin
- WebAssembly support via `experiments.asyncWebAssembly`
- Two build outputs:
  1. Main application (multi-chunk, code-split)
  2. Web component (single bundle for embedding)
- Development server with hot module replacement
- Content Security Policy enforcement in production

**Key Requirements:**
- TypeScript transpilation with type checking
- WebAssembly module loading (wasm-bindgen output)
- React Fast Refresh for development
- CSS Modules support
- Asset optimization (images, fonts)
- Source maps for debugging
- Environment variable injection
- Proxy configuration for API calls

### Documentation Site (website/)

**Current Stack:**
- Docusaurus 3.6.3
- Custom WASM plugin for WebAssembly support
- GitHub Pages deployment
- MDX support for interactive documentation
- Google Analytics integration

## Migration Strategy

### Phase 1: Parallel Rsbuild Setup (Week 1-2)

#### 1.1 Initialize Rsbuild Configuration

Create new configuration files alongside existing Webpack setup:

```
src/app/
├── config/
│   ├── webpack.config.js         (existing)
│   ├── webpack.component.config.js (existing)
│   └── rsbuild/
│       ├── rsbuild.config.ts     (new)
│       ├── rsbuild.component.config.ts (new)
│       └── shared.config.ts      (new)
```

#### 1.2 Core Rsbuild Configuration

```typescript
// src/app/config/rsbuild/shared.config.ts
import { defineConfig } from '@rsbuild/core';
import { pluginReact } from '@rsbuild/plugin-react';
import { pluginTypeCheck } from '@rsbuild/plugin-type-check';

export const sharedConfig = defineConfig({
  plugins: [
    pluginReact({
      fastRefresh: true,
    }),
    pluginTypeCheck({
      // Fork TS checker configuration
      typescript: {
        configFile: '../tsconfig.json',
      },
    }),
  ],
  source: {
    alias: {
      '@': './src',
    },
  },
  experiments: {
    asyncWebAssembly: true,
  },
  output: {
    assetPrefix: process.env.PUBLIC_URL || '/',
    distPath: {
      wasm: 'static/wasm',
    },
  },
  server: {
    proxy: {
      '/api': 'http://localhost:3030',
    },
  },
});
```

#### 1.3 Main App Configuration

```typescript
// src/app/config/rsbuild/rsbuild.config.ts
import { defineConfig } from '@rsbuild/core';
import { mergeRsbuildConfig } from '@rsbuild/core';
import { sharedConfig } from './shared.config';

export default mergeRsbuildConfig(
  sharedConfig,
  defineConfig({
    source: {
      entry: {
        main: './src/index.tsx',
      },
    },
    output: {
      distPath: {
        root: 'build-rsbuild',
      },
    },
    performance: {
      chunkSplit: {
        strategy: 'split-by-experience',
      },
    },
    security: {
      nonce: 'NONCE_PLACEHOLDER', // For CSP
    },
  })
);
```

#### 1.4 Web Component Configuration

```typescript
// src/app/config/rsbuild/rsbuild.component.config.ts
import { defineConfig } from '@rsbuild/core';
import { mergeRsbuildConfig } from '@rsbuild/core';
import { sharedConfig } from './shared.config';

export default mergeRsbuildConfig(
  sharedConfig,
  defineConfig({
    source: {
      entry: {
        'sd-component': './src/index-component.tsx',
      },
    },
    output: {
      distPath: {
        root: 'build-component-rsbuild',
      },
      filename: {
        js: 'static/js/[name].js', // Fixed name for embedding
      },
    },
    performance: {
      chunkSplit: {
        strategy: 'all-in-one', // Single bundle
      },
    },
  })
);
```

#### 1.5 Update Package Scripts

Add new scripts to `src/app/package.json`:

```json
{
  "scripts": {
    // Existing scripts remain unchanged
    "build:frontend": "node scripts/build.js",
    "build:webcomponent": "node scripts/build-component.js",
    "start:frontend": "node scripts/start.js",
    
    // New Rsbuild scripts
    "build:frontend:rsbuild": "rsbuild build -c config/rsbuild/rsbuild.config.ts",
    "build:webcomponent:rsbuild": "rsbuild build -c config/rsbuild/rsbuild.component.config.ts",
    "start:frontend:rsbuild": "rsbuild dev -c config/rsbuild/rsbuild.config.ts",
    
    // Parallel testing script
    "test:builds": "yarn build:frontend && yarn build:frontend:rsbuild && yarn compare-builds"
  }
}
```

### Phase 2: WebAssembly Integration Testing (Week 2)

#### 2.1 Verify WASM Loading

Create test script to verify WebAssembly module loading works correctly:

```typescript
// src/app/scripts/test-wasm-loading.js
const path = require('path');
const fs = require('fs');

function compareWasmBundles(webpackBuild, rsbuildBuild) {
  // Compare:
  // 1. WASM file presence and size
  // 2. Loading mechanism in JS bundles
  // 3. Initialization code
  // Return differences
}
```

#### 2.2 Browser Testing

Create automated tests to verify:
- WASM module loads correctly
- Engine functions are callable
- No performance regression
- Web component works when embedded

### Phase 3: Rspress Documentation Migration (Week 3)

#### 3.1 Initialize Rspress

```bash
cd website
npx create-rspress@latest ../website-rspress
```

#### 3.2 Migrate Configuration

```typescript
// website-rspress/rspress.config.ts
import { defineConfig } from 'rspress/config';

export default defineConfig({
  root: 'docs',
  title: 'Simlin',
  description: 'Debug your intuition',
  icon: '/img/favicon.ico',
  logo: '/img/logo.svg',
  themeConfig: {
    socialLinks: [
      {
        icon: 'github',
        mode: 'link',
        content: 'https://github.com/bpowers/simlin',
      },
    ],
  },
  builderConfig: {
    // Enable WASM support
    experiments: {
      asyncWebAssembly: true,
    },
  },
  // Google Analytics
  plugins: [
    // Add analytics plugin
  ],
});
```

#### 3.3 Content Migration

- Copy markdown files from `website/docs/` to `website-rspress/docs/`
- Convert any Docusaurus-specific MDX components
- Update internal links
- Test web component embedding in documentation

### Phase 4: Testing and Validation (Week 3-4)

#### 4.1 Build Comparison

Create comprehensive comparison tests:

1. **Bundle Size Analysis**
   - Compare total bundle sizes
   - Analyze chunk splitting effectiveness
   - Verify WASM file handling

2. **Performance Testing**
   - Build time comparison
   - Development server startup time
   - Hot module replacement speed
   - Browser runtime performance

3. **Feature Parity**
   - TypeScript type checking
   - Source maps generation
   - Asset optimization
   - Environment variable handling

#### 4.2 Integration Testing

1. **Development Workflow**
   - Verify hot reload works correctly
   - Test TypeScript error reporting
   - Validate proxy configuration

2. **Production Build**
   - Test CSP header generation
   - Verify asset paths
   - Test deployment process

### Phase 5: Migration Completion (Week 4-5)

#### 5.1 Gradual Rollout

1. Deploy Rsbuild version to staging environment
2. Run A/B tests if possible
3. Monitor for any issues
4. Gather team feedback

#### 5.2 Final Migration

Once validated:
1. Update CI/CD pipelines
2. Update documentation
3. Remove Webpack configuration files
4. Update all build scripts to use Rsbuild
5. Archive old configuration for reference

## Risk Assessment and Mitigation

### High Risk Items

1. **WebAssembly Loading Differences**
   - **Risk**: Different handling of async WASM modules
   - **Mitigation**: Extensive testing, maintain compatibility layer if needed

2. **Plugin Ecosystem**
   - **Risk**: Missing Webpack plugins not available for Rspack
   - **Mitigation**: Identify critical plugins early, find alternatives or create custom solutions

3. **Build Output Differences**
   - **Risk**: Different chunk splitting might affect loading performance
   - **Mitigation**: Careful configuration tuning, performance testing

### Medium Risk Items

1. **Development Experience**
   - **Risk**: Team needs to learn new tools
   - **Mitigation**: Create migration guide, provide training

2. **CI/CD Pipeline Updates**
   - **Risk**: Build failures in automated systems
   - **Mitigation**: Test thoroughly in CI before switching

## Success Criteria

1. **Functional Requirements**
   - ✓ Both app and web component build successfully
   - ✓ WebAssembly modules load correctly
   - ✓ TypeScript compilation and type checking work
   - ✓ Development server with HMR functions properly
   - ✓ Documentation site builds and deploys correctly

2. **Performance Requirements**
   - ✓ Build time improved by at least 30%
   - ✓ Development server startup < 3 seconds
   - ✓ No regression in bundle size (±5%)
   - ✓ Browser runtime performance maintained

3. **Developer Experience**
   - ✓ Simpler configuration files
   - ✓ Faster feedback loop in development
   - ✓ Clear error messages
   - ✓ Smooth migration path

## Timeline

- **Week 1-2**: Implement parallel Rsbuild configuration
- **Week 2**: WebAssembly integration testing
- **Week 3**: Rspress documentation migration
- **Week 3-4**: Comprehensive testing and validation
- **Week 4-5**: Final migration and cleanup

## Conclusion

This migration plan provides a safe, incremental approach to modernizing the build toolchain while maintaining the ability to rollback at any stage. The parallel setup ensures minimal disruption to ongoing development while allowing thorough testing of the new configuration.

The expected benefits include:
- Significantly faster build times (Rust-based tooling)
- Simpler, more maintainable configuration
- Better developer experience
- Future-proof toolchain with active development
- Reduced dependency on complex Webpack configurations

The migration can be executed with minimal risk by following this phased approach and maintaining the existing build system until the new one is fully validated.