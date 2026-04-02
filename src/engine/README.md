# @simlin/engine

TypeScript API for building, running, and analyzing [system dynamics](https://en.wikipedia.org/wiki/System_dynamics) models. Works in both Node.js and browsers.

Under the hood, the engine is compiled from Rust to WebAssembly. In browsers, WASM runs in a Web Worker to avoid blocking the UI thread.

## Install

```bash
npm install @simlin/engine
```

## Usage

```ts
import { Project } from '@simlin/engine';

// Load a model from XMILE XML
const project = await Project.open(xmileData);
const model = await project.mainModel();

// Run simulation with default parameters
const run = await model.run();
console.log(run.results.get('population'));

// Run with variable overrides
const overrideRun = await model.run({ birth_rate: 0.05 });

// Edit the model
await model.edit((vars, patch) => {
  patch.upsertAux({ name: 'new_var', equation: '42' });
});

// Serialize back to XMILE
const updatedXmile = await project.toXmile();
```

## Bundler Configuration (Browser)

The browser build uses a static WASM import. Your bundler needs to support WebAssembly:

- **Webpack**: Enable `experiments.asyncWebAssembly` in your webpack config
- **Vite**: Use `vite-plugin-wasm` or enable top-level await
- **Next.js**: Configure `webpack.experiments.asyncWebAssembly` in `next.config.js`

For Node.js usage, WASM is loaded from the filesystem automatically.

## License

Apache-2.0
