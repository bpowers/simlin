import {
  JupyterFrontEnd,
  JupyterFrontEndPlugin,
} from '@jupyterlab/application';

import { IRenderMime } from '@jupyterlab/rendermime-interfaces';
import { IRenderMimeRegistry } from '@jupyterlab/rendermime';

import { WidgetRenderer } from './WidgetRenderer';

import { requestAPI } from './handler';

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
    requestAPI<any>('get_example')
      .then((data) => {
        console.log(data);
      })
      .catch((reason) => {
        console.error(
          `:ohno: jupyter-simlin server extension appears to be missing.\n${reason}`,
        );
      });

    rendermime.addFactory(rendererFactory, 0);
  },
  requires: [IRenderMimeRegistry],
};

export default extension;
