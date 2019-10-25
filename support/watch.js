#!/usr/bin/env node

const browserSync = require('browser-sync');
const watch = require('watch');

const reload = browserSync.reload;

browserSync({
  port: 5000,
  notify: false,
  logPrefix: 'browsix',
  snippetOptions: {
    rule: {
      match: '<span id="browser-sync-binding"></span>',
      fn: function(snippet) {
        return snippet;
      },
    },
  },
  server: { baseDir: ['.'] },
});

// watch...
// gulp.watch(['index.html'], reload);
// gulp.watch(['src/*.ts', '!src/runtime.ts'], ['sd.js', reload]);
