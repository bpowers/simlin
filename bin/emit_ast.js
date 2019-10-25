#!/usr/bin/env node
// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

DOMParser = require('xmldom').DOMParser;
Mustache = require('mustache');

var fs = require('fs');
var sd = require('../lib/sd');

var argv = process.argv;
if (argv.length < 3) {
  console.log('usage: ./emit_sim.js XMILE_FILE');
  process.exit(1);
}

fs.readFile(argv[2], function(err, data) {
  var xml = new DOMParser().parseFromString(data.toString(), 'application/xml');
  var ctx = new sd.Project(xml);
  var mdl = ctx.model();
  // console.log(mdl);
  console.log(JSON.stringify(ctx.files[0], null, '    '));
});
