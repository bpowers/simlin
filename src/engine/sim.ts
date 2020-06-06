// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Map } from 'immutable';

import { defined } from './common';
import { buildSim } from './sim-builder';
import * as vars from './vars';

export interface SeriesProps {
  name: string;
  time: Float64Array;
  values: Float64Array;
}
export type Series = Readonly<SeriesProps>;

export class Sim {
  root: vars.Module;
  project: vars.Project;
  seq = 1; // unique message ids
  // callback storage, keyed by message id
  promised: Map<number, (result: any, err: any) => void> = Map();
  worker?: Worker;

  constructor(project: vars.Project, root: vars.Module, isStandalone: boolean) {
    this.root = root;
    this.project = project;

    const worker = buildSim(project, root, isStandalone);
    if (!worker) {
      return;
    }

    this.worker = worker;
    this.worker.addEventListener('message', (e: MessageEvent): void => {
      const id = e.data[0] as number;
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const result = e.data[1];
      const cb = this.promised.get(id);
      this.promised = this.promised.delete(id);
      if (cb) {
        cb(result[0], result[1]);
      }
    });
  }

  private post<T>(...args: any[]): Promise<T> {
    const id = this.seq++;

    return new Promise<T>((resolve, reject) => {
      if (!this.worker) {
        return;
      }
      this.promised = this.promised.set(id, (result: any, err: any) => {
        if (err !== undefined && err !== null) {
          reject(err);
        } else {
          resolve(result);
        }
      });
      this.worker.postMessage([id].concat(args));
    });
  }

  close(): void {
    if (!this.worker) {
      return;
    }
    this.worker.terminate();
    this.worker = undefined;
  }

  reset(): Promise<void> {
    return this.post('reset');
  }

  setValue(name: string, val: number): Promise<void> {
    return this.post('set_val', name, val);
  }

  async value(...names: string[]): Promise<Map<string, number>> {
    const args = ['get_val'].concat(names);
    const values: Iterable<[string, number]> = await this.post(...args);
    return Map<string, number>(values);
  }

  async series(...names: string[]): Promise<Map<string, Series>> {
    const args = ['get_series'].concat(names);
    const series: Iterable<[string, Series]> = await this.post(...args);
    return Map<string, Series>(series);
  }

  dominance(overrides: { [n: string]: number }, indicators: string[]): Promise<{ [name: string]: number }> {
    return this.post('dominance', overrides, indicators);
  }

  runTo(time: number): Promise<number> {
    return this.post('run_to', time);
  }

  runToEnd(): Promise<number> {
    return this.post('run_to_end');
  }

  varNames(includeHidden = false): Promise<string[]> {
    return this.post('var_names', includeHidden);
  }

  async csv(delim = ','): Promise<string> {
    const names = await this.varNames();
    const data = await this.series(...names);

    return Sim.csvFromData(data, names, delim);
  }

  private static csvFromData(data: Map<string, Series>, vars: string[], delim: string): string {
    let file = '';
    const series: { [name: string]: Series } = {};
    let time: Series | undefined;
    let header = 'time' + delim;

    // create the CSV header
    for (const v of vars) {
      if (v === 'time') {
        time = data.get(v);
        continue;
      }
      if (!data.has(v)) {
        continue;
      }
      header += v + delim;
      series[v] = defined(data.get(v));
    }

    if (time === undefined) {
      throw new Error('no time?');
    }

    file += header.substr(0, header.length - 1);
    file += '\n';

    // now go timestep-by-timestep to generate each line
    const nSteps = time.values.length;
    for (let i = 0; i < nSteps; i++) {
      let msg = '';
      for (const v in series) {
        if (!series.hasOwnProperty(v)) {
          continue;
        }
        if (msg === '') {
          msg += `${series[v].time[i]}${delim}`;
        }
        msg += `${series[v].values[i]}${delim}`;
      }
      file += msg.substr(0, msg.length - 1);
      file += '\n';
    }

    return file;
  }
}
