import React from 'react';

import { Project } from '@system-dynamics/core/datamodel';
import { renderSvgToString } from '@system-dynamics/diagram';
import { defined } from '@system-dynamics/core/common';
import { Editor } from '@system-dynamics/diagram';
import { fromXmile, toXmile } from "@system-dynamics/importer";
import { convertMdlToXmile } from '@system-dynamics/xmutil';

import { fromBase64, fromUint8Array, toUint8Array } from 'js-base64';

import { ReactWidget } from '@jupyterlab/apputils';

import { IRenderMime } from '@jupyterlab/rendermime-interfaces';

import { requestAPI } from './handler';

const CLASS_NAME = 'mimerenderer-simlin_jupyter_widget';

export class WidgetRenderer extends ReactWidget implements IRenderMime.IRenderer {
  constructor() {
    super();
    this.addClass(CLASS_NAME);
  }

  private project?: Uint8Array;
  private projectId = '';
  private isEditable = false;

  async renderModel(mimeModel: IRenderMime.IMimeModel): Promise<void> {
    const source: any = mimeModel.data[this.mimeType];

    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const projectId: string = source['project_id'];
    this.projectId = projectId;

    let isSimlin = false;
    let contents = '';
    try {
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const data = await requestAPI<any>('model/' + encodeURIComponent(projectId));
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      contents = data['contents'];
      isSimlin = true;
    } catch (_err) {
      // FIXME
    }

    this.isEditable = !!source['project_is_editable'];

    // if the request above 404'd, then start with the initial source
    if (contents === '') {
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      contents = source['project_initial_source'];
    }
    if (projectId.endsWith('.mdl.simlin') && !isSimlin) {
      contents = await convertMdlToXmile(fromBase64(contents), false);
      this.project = await fromXmile(contents);
    } else if (!isSimlin && (projectId.endsWith('.stmx.simlin') || projectId.endsWith('.xmile.simlin'))) {
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
  handleSave = async (project: Readonly<Uint8Array>, currVersion: number): Promise<number | undefined> => {
    const xmile = await toXmile(project);
    const body = {
      contents: fromUint8Array(project as Uint8Array),
      xmile,
    };

    await requestAPI<any>('model/' + encodeURIComponent(this.projectId), body);

    return currVersion + 1; // or whatever
  };

  render(): React.ReactElement {
    if (!this.project) {
      return <div />;
    }
    const style = this.isEditable ? { height: 625 } : undefined;
    return (
      <div style={style}>
        <Editor
          initialProjectBinary={defined(this.project)}
          initialProjectVersion={1}
          name={'model'}
          embedded={!this.isEditable}
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
