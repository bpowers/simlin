import React from 'react';

import { Project } from '@system-dynamics/core/datamodel';
import { renderSvgToString } from '@system-dynamics/diagram';
import { defined } from '@system-dynamics/core/common';
import { Editor } from '@system-dynamics/diagram';
import { fromXmile } from '@system-dynamics/importer';
import { convertMdlToXmile } from '@system-dynamics/xmutil';

import { fromBase64, toUint8Array } from 'js-base64';

import { ReactWidget } from '@jupyterlab/apputils';

import { IRenderMime } from '@jupyterlab/rendermime-interfaces';

const CLASS_NAME = 'mimerenderer-simlin_jupyter_widget';

export class WidgetRenderer extends ReactWidget implements IRenderMime.IRenderer {
  constructor() {
    super();
    this.addClass(CLASS_NAME);
  }

  project?: Uint8Array;

  async renderModel(mimeModel: IRenderMime.IMimeModel): Promise<void> {
    const source: any = mimeModel.data[this.mimeType];

    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const projectId: string = source['project_id'];
    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    let contents = source['project_source'];
    if (projectId.endsWith('.mdl')) {
      contents = await convertMdlToXmile(fromBase64(contents), false);
      this.project = await fromXmile(contents);
    } else if (projectId.endsWith('.stmx') || projectId.endsWith('.xmile')) {
      this.project = await fromXmile(fromBase64(contents));
    } else {
      this.project = toUint8Array(contents);
    }
    this.update();

    const project = defined(Project.deserializeBinary(this.project));
    try {
      const [svg] = renderSvgToString(project, 'main');

      if (!mimeModel.data['image/svg+xml']) {
        setTimeout(() => {
          mimeModel.setData({
            data: Object.assign({}, mimeModel.data, {
              'image/svg+xml': svg,
            }),
            metadata: mimeModel.metadata,
          });
        });
      }
    } catch (_err) {
      // do nothing; this is broken in development (and if it fails, no big deal)
    }
  }

  // eslint-disable-next-line @typescript-eslint/require-await
  handleSave = async (_project: Readonly<Uint8Array>, _currVersion: number): Promise<number | undefined> => {
    return undefined;
  };

  render(): React.ReactElement {
    console.log('render called');
    if (!this.project) {
      return <div />;
    }
    return (
      <div style={{ height: 625 }}>
        <Editor
          initialProjectBinary={defined(this.project)}
          initialProjectVersion={1}
          embedded={false}
          onSave={this.handleSave}
        />
      </div>
    );
  }

  /**
   * The mimetype being rendered.
   */
  readonly mimeType = 'application/vnd.simlin.widget-view+json';
}
