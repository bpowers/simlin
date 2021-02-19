import { JupyterFrontEnd, JupyterFrontEndPlugin } from '@jupyterlab/application';

import { IRenderMime } from '@jupyterlab/rendermime-interfaces';
import { IRenderMimeRegistry } from '@jupyterlab/rendermime';

import { WidgetRenderer } from './WidgetRenderer';

/**
 * The mime type for a widget view.
 */
export const MIME_TYPE = 'application/vnd.simlin.widget-view+json';

export const rendererFactory: IRenderMime.IRendererFactory = {
  safe: true,
  mimeTypes: [MIME_TYPE],
  createRenderer: () => new WidgetRenderer(),
};

/**
 * Initialization data for the jupyter-simlin extension.
 */
const extension: JupyterFrontEndPlugin<void> = {
  id: 'jupyter-simlin:plugin',
  autoStart: true,
  activate: (app: JupyterFrontEnd, rendermime: IRenderMimeRegistry) => {
    rendermime.addFactory(rendererFactory, 0);
  },
  requires: [IRenderMimeRegistry],
};

export default extension;
