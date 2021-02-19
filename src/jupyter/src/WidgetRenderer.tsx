import React from 'react';

import { datamodel } from '@system-dynamics/core';
import { renderSvgToString } from '@system-dynamics/diagram';
import { fromXmile } from '@system-dynamics/importer';
import { convertMdlToXmile } from '@system-dynamics/xmutil';

import { fromBase64 } from 'js-base64';

import { ReactWidget } from '@jupyterlab/apputils';

import { IRenderMime } from '@jupyterlab/rendermime-interfaces';

const CLASS_NAME = 'mimerenderer-simlin_jupyter_widget';

export class WidgetRenderer
  extends ReactWidget
  implements IRenderMime.IRenderer {
  constructor() {
    super();
    this.addClass(CLASS_NAME);
  }

  async renderModel(mimeModel: IRenderMime.IMimeModel): Promise<void> {
    // const source: any = mimeModel.data[this.mimeType];
    //
    // const projectId: string = source['project_id'];
    // let contents = source['project_source'];
    // let project: datamodel.Project;
    // if (projectId.endsWith('.mdl')) {
    //   contents = await convertMdlToXmile(fromBase64(contents), false);
    //   const pb = await fromXmile(contents);
    //   project = datamodel.Project.deserializeBinary(pb);
    // } else if (projectId.endsWith('.stmx') || projectId.endsWith('.xmile')) {
    //   const pb = await fromXmile(fromBase64(contents));
    //   project = datamodel.Project.deserializeBinary(pb);
    // } else {
    //   project = datamodel.Project.deserializeBase64(contents);
    // }
    //
    // const [svg] = await renderSvgToString(project, 'main');
    //
    // if (!mimeModel.data['image/svg+xml']) {
    //   setTimeout(() => {
    //     mimeModel.setData({
    //       data: Object.assign({}, mimeModel.data, {
    //         'image/svg+xml': svg,
    //       }),
    //       metadata: mimeModel.metadata,
    //     });
    //   });
    // }
    //
    // return Promise.resolve();
  }

  render() {
    return <p>womp womp</p>;
  }

  /**
   * The mimetype being rendered.
   */
  readonly mimeType = 'application/vnd.simlin.widget-view+json';
}
