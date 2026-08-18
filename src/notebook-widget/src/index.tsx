// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * anywidget front-end module (AFM) entry: `export default { initialize, render }`.
 *
 * `initialize` runs once per widget instance and only kicks off the engine
 * bootstrap so the wasm request is on the wire as early as possible; it must
 * return quickly because anywidget rejects a model whose module load plus
 * initialize exceed ~2s (runtime.ts `AbortSignal.timeout(2000)`), after which
 * every comm message for that model is dropped. `render` runs once per view:
 * it mounts React into `el`, shows a placeholder until the engine is ready,
 * then hands off to WidgetApp.
 */

import * as React from 'react';
import { createRoot } from 'react-dom/client';

// The package root of @simlin/diagram would also pull in reset.css (a global
// page reset -- unacceptable inside someone else's notebook page), so the
// Editor is deep-imported and the two stylesheets the Editor needs are
// imported here explicitly. Both are injected into the document at module
// load by the bundle (output.injectStyles).
import '@simlin/diagram/theme.css';
import 'katex/dist/katex.min.css';

import type { InitializeContext, RenderContext } from './anywidget-model';
import { ensureEngine } from './engine-bootstrap';
import { WIDGET_ROOT_CLASS, WidgetApp } from './WidgetApp';
import styles from './widget.module.css';
import { readTraits, wrapperStyle } from './widget-core';

/** The `name` the Editor uses for downloads; the widget has one project. */
const PROJECT_NAME = 'model';

function Placeholder({
  height,
  text,
  isError,
}: {
  height: number;
  text: string;
  isError: boolean;
}): React.ReactElement {
  return (
    <div className={`${WIDGET_ROOT_CLASS} ${styles.host}`} style={wrapperStyle(height)}>
      <div className={isError ? `${styles.placeholder} ${styles.placeholderError}` : styles.placeholder} role="status">
        {text}
      </div>
    </div>
  );
}

function initialize({ model }: InitializeContext): void {
  // Fire-and-forget: render() awaits the same memoized promise and surfaces a
  // failure there, in the cell, where the user can see it.
  ensureEngine(model).catch(() => undefined);
}

async function render({ model, el, signal }: RenderContext): Promise<() => void> {
  const mount = document.createElement('div');
  el.appendChild(mount);
  const root = createRoot(mount);
  const cleanup = (): void => {
    root.unmount();
    mount.remove();
  };

  const initialHeight = readTraits((key) => model.get(key)).height;
  root.render(<Placeholder height={initialHeight} text="Loading the Simlin engine..." isError={false} />);

  try {
    await ensureEngine(model);
  } catch (err) {
    if (signal?.aborted) {
      cleanup();
      return () => undefined;
    }
    const message = err instanceof Error ? err.message : String(err);
    root.render(<Placeholder height={initialHeight} text={`Simlin widget failed to start: ${message}`} isError />);
    return cleanup;
  }
  if (signal?.aborted) {
    cleanup();
    return () => undefined;
  }

  root.render(<WidgetApp model={model} name={PROJECT_NAME} />);
  return cleanup;
}

export default { initialize, render };
