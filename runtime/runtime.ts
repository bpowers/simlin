// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.
'use strict';

// 'time' is always offset 0.  We used to have var named 'TIME', but
// now we inline '0' below.

function i32(n: number): number {
  'use strict';
  return n | 0;
}

// copied from src/i.ts
interface Table {
  x: number[];
  y: number[];
}

interface SimSpec {
  start: number;
  stop: number;
  dt: number;
  saveStep: number;
  method: string;
  timeUnits: string;
}

interface Series {
  name: string;
  time: Float64Array;
  values: Float64Array;
}

interface CalcFn {
  (dt: number, curr: Float64Array): void;
}

interface CalcStocksFn {
  (dt: number, curr: Float64Array, next: Float64Array): void;
}

class Simulation {
  name: string;
  _shift: number;

  parent: Simulation;

  saveEvery: number;
  stepNum: number;
  nVars: number;

  modules: { [name: string]: Simulation };
  symRefs: { [name: string]: string };
  implicitRefs: [string];
  ref: { [name: string]: number };

  initials: { [name: string]: number };
  simSpec: SimSpec;
  offsets: { [name: string]: number };
  tables: { [name: string]: Table };

  slab: Float64Array;

  calcInitial: CalcFn;
  calcFlows: CalcFn;
  calcStocks: CalcStocksFn;

  lookupOffset(id: string): number {
    if (id === 'time') {
      return 0;
    }
    if (id[0] === '.') {
      id = id.substr(1);
    }
    if (id in this.offsets) {
      return this._shift + this.offsets[id];
    }
    let parts = id.split('.');
    if (parts.length === 1 && id === '' && this.name in this.offsets) {
      return this._shift + this.offsets[this.name];
    }
    const nextSim = this.modules[parts[0]];
    if (!nextSim) {
      return -1;
    }
    return nextSim.lookupOffset(parts.slice(1).join('.'));
  }

  root(): Simulation {
    if (!this.parent) {
      return this;
    }
    return this.parent.root();
  }

  resolveAllSymbolicRefs(): void {
    for (let n in this.symRefs) {
      if (!this.symRefs.hasOwnProperty(n)) {
        continue;
      }
      let ctx: any;
      if (this.symRefs[n][0] === '.' || this === this.root()) {
        ctx = this.root();
      } else {
        ctx = this.parent;
      }
      this.ref[n] = ctx.lookupOffset(this.symRefs[n]);
    }
    for (let n in this.modules) {
      if (!this.modules.hasOwnProperty(n)) {
        continue;
      }
      this.modules[n].resolveAllSymbolicRefs();
    }
    for (const n of this.implicitRefs) {
      this.ref[n] = this.lookupOffset(n);
    }
  }

  varNames(includeHidden: boolean): string[] {
    let result = Object.keys(this.offsets).filter(v => includeHidden || !v.startsWith('$·'));
    for (let v in this.modules) {
      if (!this.modules.hasOwnProperty(v)) {
        continue;
      }
      if (!includeHidden && v.startsWith('$·')) {
        continue;
      }
      let ids: string[] = [];
      let modVarNames = this.modules[v].varNames(includeHidden);
      for (let n in modVarNames) {
        if (modVarNames.hasOwnProperty(n)) {
          ids.push(v + '.' + modVarNames[n]);
        }
      }
      result = result.concat(ids);
    }
    if (this.name === 'main') {
      result.push('time');
    }

    return result;
  }

  getNVars(): number {
    let nVars = Object.keys(this.offsets).length;
    for (let n in this.modules) {
      if (this.modules.hasOwnProperty(n)) {
        nVars += this.modules[n].getNVars();
      }
    }
    // if we're main, claim time
    if (this.name === 'main') {
      nVars++;
    }
    return nVars;
  }

  reset(): void {
    const spec = this.simSpec;
    const nSaveSteps = i32((spec.stop - spec.start) / spec.saveStep + 1);

    this.stepNum = 0;

    this.slab = new Float64Array(this.nVars * (nSaveSteps + 1));

    let curr = this.curr();
    curr[0 /*TIME*/] = spec.start;
    this.saveEvery = Math.max(1, i32(spec.saveStep / spec.dt + 0.5));

    this.calcInitial(this.simSpec.dt, curr);
  }

  dominance(forced: { [name: string]: number }, indicators: string[]): { [name: string]: number } {
    const dt = this.simSpec.dt;

    // both slices so that we don't modify existing data
    let curr = this.curr().slice();
    let next = new Float64Array(curr.length);

    // override values in the current timestep
    for (let name in forced) {
      if (!forced.hasOwnProperty(name)) {
        continue;
      }

      let off = this.lookupOffset(name);
      if (off === -1) {
        console.log(`WARNING: variable '${name}' not found.`);
        return {};
      }
      curr[off] = forced[name];
    }

    this.calcFlows(dt, curr);
    this.calcStocks(dt, curr, next);

    next[0 /*TIME*/] = curr[0 /*TIME*/] + dt;

    // copy the specified indicators into an object and return it.
    let result: { [name: string]: number } = {};
    for (let i = 0; i < indicators.length; i++) {
      let name = indicators[i];
      let off = this.lookupOffset(name);
      if (off === -1) {
        console.log(`WARNING: variable '${name}' not found.`);
        continue;
      }
      result[name] = next[off];
    }

    return result;
  }

  runTo(endTime: number): void {
    const dt = this.simSpec.dt;

    let curr = this.curr();
    let next = this.slab.subarray((this.stepNum + 1) * this.nVars, (this.stepNum + 2) * this.nVars);

    while (curr[0 /*TIME*/] <= endTime) {
      this.calcFlows(dt, curr);
      this.calcStocks(dt, curr, next);

      next[0 /*TIME*/] = curr[0 /*TIME*/] + dt;

      if (this.stepNum++ % this.saveEvery !== 0) {
        curr.set(next);
      } else {
        curr = next;
        next = this.slab.subarray(
          (i32(this.stepNum / this.saveEvery) + 1) * this.nVars,
          (i32(this.stepNum / this.saveEvery) + 2) * this.nVars,
        );
      }
    }
  }

  runToEnd(): void {
    return this.runTo(this.simSpec.stop + 0.5 * this.simSpec.dt);
  }

  curr(): Float64Array {
    return this.slab.subarray(this.stepNum * this.nVars, (this.stepNum + 1) * this.nVars);
  }

  setValue(name: string, value: number): void {
    const off = this.lookupOffset(name);
    if (off === -1) {
      return;
    }
    this.curr()[off] = value;
  }

  value(name: string): number {
    const off = this.lookupOffset(name);
    if (off === -1) {
      return NaN;
    }
    const saveNum = i32(this.stepNum / this.saveEvery);
    const slabOff = this.nVars * saveNum;
    return this.slab.subarray(slabOff, slabOff + this.nVars)[off];
  }

  series(name: string): Series | null {
    const saveNum = i32(this.stepNum / this.saveEvery);
    const time = new Float64Array(saveNum);
    const values = new Float64Array(saveNum);
    const off = this.lookupOffset(name);
    if (off === -1) {
      return null;
    }
    for (let i = 0; i < time.length; i++) {
      let curr = this.slab.subarray(i * this.nVars, (i + 1) * this.nVars);
      time[i] = curr[0];
      values[i] = curr[off];
    }
    return {
      name: name,
      time: time,
      values: values,
    };
  }
}

let cmds: any;

function handleMessage(e: any): void {
  'use strict';

  let id = e.data[0];
  let cmd = e.data[1];
  let args = e.data.slice(2);
  let result: [any, any];

  if (cmds.hasOwnProperty(cmd)) {
    result = cmds[cmd].apply(null, args);
  } else {
    result = [null, 'unknown command "' + cmd + '"'];
  }

  if (!Array.isArray(result)) {
    result = [null, 'no result for [' + e.data.join(', ') + ']'];
  }

  // TODO(bp) look into transferrable objects
  let msg = [id, result];

  (<DedicatedWorkerGlobalScope>self).postMessage(msg);
}

function initCmds(main: Simulation): any {
  'use strict';

  return {
    reset: function(): [undefined, undefined] | [undefined | Error] {
      main.reset();
      return [undefined, undefined];
    },
    set_val: function(name: string, val: number): [undefined, undefined] | [undefined | Error] {
      main.setValue(name, val);
      return [undefined, undefined];
    },
    get_val: function(...args: string[]): [{ [name: string]: number }, undefined] | [undefined, Error] {
      let result: { [name: string]: number } = {};
      for (let i = 0; i < args.length; i++) {
        result[args[i]] = main.value(args[i]);
      }
      return [result, undefined];
    },
    get_series: function(...args: string[]): [{ [name: string]: Series }, undefined] | [undefined, Error] {
      let result: { [name: string]: Series } = {};
      for (let i = 0; i < args.length; i++) {
        let series = main.series(args[i]);
        if (series) {
          result[args[i]] = series;
        }
      }
      return [result, undefined];
    },
    dominance: function(
      overrides: { [n: string]: number },
      indicators: string[],
    ): [{ [name: string]: number }, undefined] | [undefined, Error] {
      return [main.dominance(overrides, indicators), undefined];
    },
    run_to: function(time: number): [number, undefined] | [undefined, Error] {
      main.runTo(time);
      return [main.value('time'), undefined];
    },
    run_to_end: function(): [number, undefined] | [undefined, Error] {
      main.runToEnd();
      return [main.value('time'), undefined];
    },
    var_names: function(includeHidden: boolean): [string[], undefined] | [undefined, Error] {
      return [main.varNames(includeHidden), undefined];
    },
  };
}

function lookup(table: any, index: number): number {
  'use strict';

  const size = table.x.length;
  if (size === 0) {
    return NaN;
  }

  const x = table.x;
  const y = table.y;

  if (index <= x[0]) {
    return y[0];
  } else if (index >= x[size - 1]) {
    return y[size - 1];
  }

  // binary search seems to be the most appropriate choice here.
  let low = 0;
  let high = size;
  let mid: number;
  while (low < high) {
    mid = Math.floor(low + (high - low) / 2);
    if (x[mid] < index) {
      low = mid + 1;
    } else {
      high = mid;
    }
  }

  let i = low;
  if (x[i] === index) {
    return y[i];
  } else {
    // slope = deltaY/deltaX
    const slope = (y[i] - y[i - 1]) / (x[i] - x[i - 1]);
    // y = m*x + b
    return (index - x[i - 1]) * slope + y[i - 1];
  }
}

function abs(a: number): number {
  a = +a;
  return Math.abs(a);
}

function arccos(a: number): number {
  a = +a;
  return Math.acos(a);
}

function arcsin(a: number): number {
  a = +a;
  return Math.asin(a);
}

function arctan(a: number): number {
  a = +a;
  return Math.atan(a);
}

function cos(a: number): number {
  a = +a;
  return Math.cos(a);
}

function exp(a: number): number {
  a = +a;
  return Math.exp(a);
}

function inf(): number {
  return Infinity;
}

function int(a: number): number {
  a = +a;
  return a | 0;
}

function ln(a: number): number {
  a = +a;
  return Math.log(a);
}

function log10(a: number): number {
  a = +a;
  return Math.log10(a);
}

function max(a: number, b: number): number {
  a = +a;
  b = +b;
  return a > b ? a : b;
}

function min(a: number, b: number): number {
  a = +a;
  b = +b;
  return a < b ? a : b;
}

function pi(): number {
  return Math.PI;
}

function pulse(dt: number, time: number, volume: number, firstPulse: number, interval: number): number {
  if (time < firstPulse) return 0;
  let nextPulse = firstPulse;
  while (time >= nextPulse) {
    if (time < nextPulse + dt) {
      return volume / dt;
    } else if (interval <= 0.0) {
      break;
    } else {
      nextPulse += interval;
    }
  }
  return 0;
}

function safediv(a: number, b: number, alternative?: number): number {
  a = +a;
  b = +b;

  if (b !== 0) {
    return a / b;
  }

  return alternative ? alternative : 0;
}

function sin(a: number): number {
  a = +a;
  return Math.sin(a);
}

function sqrt(a: number): number {
  a = +a;
  return Math.sqrt(a);
}

function tan(a: number): number {
  a = +a;
  return Math.tan(a);
}
