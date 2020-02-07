// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List, Map, Record } from 'immutable';

import * as stdlib from './stdlib';

import { Error } from './common';
import { Model } from './model';
import { defined } from './util';
import { Module, Project as varsProject } from './vars';
import { File, FileFromJSON, Model as XmileModel, SimSpec, Variable as XmileVariable, ViewElementType } from './xmile';

const getXmileElement = (xmileDoc: XMLDocument): Element | undefined => {
  // in Chrome/Firefox, item 0 is xmile.  Under node's XML DOM
  // item 0 is the <xml> prefix.  And I guess there could be
  // text nodes in there, so just explictly look for xmile
  for (let i = 0; i < xmileDoc.childNodes.length; i++) {
    const node = xmileDoc.childNodes.item(i) as Element;
    if (node.tagName === 'xmile') {
      return node;
    }
  }
  return undefined;
};

const projectDefaults = {
  name: 'sd project',
  main: undefined as Module | undefined,
  files: List<File>(),
  models: Map<string, Model>(),
};

/**
 * Project is the container for a set of SD models.
 *
 * A single project may include models + non-model elements
 */
export class Project extends Record(projectDefaults) implements varsProject {
  constructor(omitStdlib = false) {
    let models = Map<string, Model>();

    if (!omitStdlib) {
      // eslint-disable-next-line
      for (const [_name, modelJSON] of stdlib.xmileModels) {
        let file;
        try {
          file = FileFromJSON(modelJSON);
        } catch (err) {
          throw err;
        }
        if (file.models.size !== 1) {
          throw new Error(`stdlib layout error`);
        }

        const xModel = defined(file.models.first());
        const model = new Model((undefined as any) as Project, xModel);
        if (!model.ident.startsWith('stdlib·')) {
          throw new Error(`stdlib bad model name: ${model.ident}`);
        }
        models = models.set(model.ident, model);
      }
    }
    super({ models });
  }

  setSimSpec(simSpec: SimSpec): Project {
    const updatePath = ['files', 0];
    return this.updateIn(updatePath, (file: File) => {
      return file.set('simSpec', simSpec);
    });
  }

  get simSpec(): SimSpec {
    return defined(defined(this.files.last()).simSpec);
  }

  toFile(): File {
    if (this.files.size !== 1) {
      throw new Error('Expected only a single file in a project for now');
    }
    const file = defined(this.files.get(0));
    let models = List<XmileModel>();
    for (const [_name, model] of this.models) {
      if (!model.shouldPersist) {
        continue;
      }
      models = models.push(model.toXmile());
    }
    return file.set('models', models);
  }

  isSimulatable(modelName: string): boolean {
    const model = this.model(modelName);
    if (!model) {
      return false;
    }

    for (const [_, variable] of model.vars) {
      if (variable.errors.size > 0) {
        return false;
      }
    }

    return true;
  }

  addNewVariable(modelName: string, type: ViewElementType, name: string): Project {
    let model = this.model(modelName);
    if (!model) {
      console.log(`setEquation: unknown model ${modelName}`);
      return this;
    }

    model = model.addNewVariable(this, type, name);

    return this.set('models', this.models.set(modelName, model));
  }

  deleteVariables(modelName: string, names: readonly string[]): Project {
    let model = this.model(modelName);
    if (!model) {
      console.log(`setEquation: unknown model ${modelName}`);
      return this;
    }

    model = model.deleteVariables(this, names);

    return this.set('models', this.models.set(modelName, model));
  }

  removeStocksFlow(modelName: string, stock: string, flow: string, dir: 'in' | 'out'): Project {
    let model = this.model(modelName);
    if (!model) {
      console.log(`setEquation: unknown model ${modelName}`);
      return this;
    }

    model = model.removeStocksFlow(this, stock, flow, dir);

    return this.set('models', this.models.set(modelName, model));
  }

  addStocksFlow(modelName: string, stock: string, flow: string, dir: 'in' | 'out'): Project {
    let model = this.model(modelName);
    if (!model) {
      console.log(`setEquation: unknown model ${modelName}`);
      return this;
    }

    model = model.addStocksFlow(this, stock, flow, dir);

    return this.set('models', this.models.set(modelName, model));
  }

  setEquation(modelName: string, ident: string, newEquation: string): Project {
    let model = this.model(modelName);
    if (!model) {
      console.log(`setEquation: unknown model ${modelName}`);
      return this;
    }

    model = model.setEquation(this, ident, newEquation);

    return this.set('models', this.models.set(modelName, model));
  }

  rename(modelName: string, oldName: string, newName: string): Project {
    let model = this.model(modelName);
    if (!model) {
      console.log(`rename: unknown model ${modelName}`);
      return this;
    }

    model = model.rename(this, oldName, newName);

    return this.set('models', this.models.set(modelName, model));
  }

  model(name?: string): Model | undefined {
    if (!name) {
      name = 'main';
    }
    if (this.models.has(name)) {
      return this.models.get(name);
    }

    return this.models.get('stdlib·' + name);
  }

  addXmileFile(xmileDoc: XMLDocument, isMain = false): [Project, undefined] | [undefined, Error] {
    const [file, err] = Project.parseFile(xmileDoc);
    if (err) {
      return [undefined, err];
    }

    return this.addFile(defined(file), isMain);
  }

  addFile(file: File, isMain = false): [Project, undefined] | [undefined, Error] {
    const xModels = file.models.map(model => model.fixupClouds());
    file.set('models', xModels);
    const files = this.files.push(file);

    // FIXME: merge the other parts of the model into the project
    const models = Map(
      xModels.map((xModel): [string, Model] => {
        const model = new Model(this, xModel, true);
        return [model.ident, model];
      }),
    );

    let dupErr: Error | undefined;
    models.forEach((model, name) => {
      if (this.models.has(name)) {
        dupErr = new Error(`duplicate name ${name}`);
      }
    });
    if (dupErr) {
      return [undefined, dupErr];
    }

    const xMod = new XmileVariable({
      type: 'module',
      name: 'main',
    });
    const main = new Module(xMod);

    let newProject = this.mergeDeep({
      files,
      models: this.models.merge(models),
      main,
    });

    if (models.has('main') && defined(file).header && defined(defined(file).header).name) {
      newProject = newProject.set('name', defined(defined(file).header).name);
    }

    return [newProject, undefined];
  }

  // isMain should only be true when called from the constructor.
  private static parseFile(xmileDoc: XMLDocument): [File, undefined] | [undefined, Error] {
    if (!xmileDoc || xmileDoc.getElementsByTagName('parsererror').length !== 0) {
      return [undefined, Error.Version];
    }
    const xmileElement = getXmileElement(xmileDoc);
    if (!xmileElement) {
      return [undefined, new Error('no XMILE root element')];
    }

    // FIXME: compat translation of XML

    // finished with XMLDocument at this point, we now
    // have a tree of native JS objects with a 1:1
    // correspondence to the XMILE doc
    const [file, err] = File.FromXML(xmileElement);
    if (err || !file) {
      return [undefined, new Error(`File.Build: ${err}`)];
    }

    // FIXME: compat translation of equations

    return [file, undefined];
  }
}

// a project consisting of all the standard library modules
export const stdProject: Project = new Project();
