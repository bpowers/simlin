// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.
/// <reference path="./generated.d.ts" />

declare var print: (msg: string) => void;

let pr: (...args: string[]) => void;

if (typeof console === 'undefined') {
  pr = print;
} else {
  pr = console.log;
}

main.runToEnd();
const series: { [name: string]: Series } = {};
let header = 'time\t';
const vars = main.varNames(false);
vars.sort();
for (let i = 0; i < vars.length; i++) {
  const v = vars[i];
  if (v === 'time') {
    continue;
  }
  header += v + '\t';
  const s = main.series(v);
  if (s) {
    series[v] = s;
  }
}
pr(header.substr(0, header.length - 1));

let nSteps = 0;
let timeSeries = main.series('time');
if (timeSeries !== null) nSteps = timeSeries.time.length;
for (let i = 0; i < nSteps; i++) {
  let msg = '';
  for (const v in series) {
    if (!series.hasOwnProperty(v)) {
      continue;
    }
    if (msg === '') {
      msg += series[v].time[i] + '\t';
    }
    msg += series[v].values[i] + '\t';
  }
  pr(msg.substr(0, msg.length - 1));
}
