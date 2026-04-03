# @simlin/diagram

React components for [system dynamics](https://en.wikipedia.org/wiki/System_dynamics) model visualization and editing. Provides a full-featured stock-and-flow diagram editor alongside a Material-inspired UI component library.

## Install

```bash
npm install @simlin/diagram @simlin/engine @simlin/core
```

**Peer dependencies**: React 19 is required.

```bash
npm install react react-dom
```

## Usage

```tsx
import { Editor } from '@simlin/diagram';

function App() {
  return (
    <Editor
      projectData={projectData}
      onProjectDataChange={handleChange}
    />
  );
}
```

The package also exports 40+ UI components (`Button`, `Dialog`, `Drawer`, `Tabs`, etc.) and an icon library.

## Styling

The package imports a CSS reset and defines CSS custom properties for theming. Your bundler must support CSS imports. CSS Modules (`.module.css`) are used for component-scoped styles.

Dark mode is supported via `[data-theme="dark"]` on a parent element.

## License

Apache-2.0
