#!/usr/bin/env node
// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

const fs = require('fs');
const sd = require('../lib/sd');

const DOMParser = require('xmldom').DOMParser;
const promisify = require('util').promisify;
const readFile = promisify(fs.readFile);

const main = async () => {
  const argv = process.argv;
  if (argv.length < 3) {
    console.log('usage: ./emit_sim.js XMILE_FILE');
    process.exit(1);
  }

  const data = await readFile(argv[2]);
  const xml = new DOMParser().parseFromString(data.toString(), 'application/xml');
  const [project, err] = sd.stdProject.addXmileFile(xml);
  if (err) {
    throw err;
  }
  // called for the side effect
  new sd.Sim(project, project.main, true);
};

main();
