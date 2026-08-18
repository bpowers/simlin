// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The stable, un-hashed class on the widget's root element. The build scopes
 * every global stylesheet (theme.css tokens, katex) under this selector so
 * nothing leaks into the notebook page (build/scope-css.ts), and the shell
 * puts it on the wrapper. Its own module so rsbuild.config.ts can import it
 * without pulling React in.
 */
export const WIDGET_ROOT_CLASS = 'simlin-notebook-widget';
