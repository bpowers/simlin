// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Global stylesheets ride on the package root because it is the one module
// every consumer executes. theme.css defines the var(--*) tokens every
// component stylesheet resolves against — without it, declarations like
// `fill: var(--color-white)` are invalid at computed-value time and SVG
// shapes render with the initial fill (black).
//
// These imports only survive a consumer's production build because
// package.json `sideEffects` lists the compiled entry (lib*/index.js) in
// addition to the CSS globs: the array means "ONLY these files have side
// effects", so an unlisted entry is declared pure and bundlers re-export
// from it without including its body — silently dropping bare CSS imports
// like these (tests/theme-tokens.test.ts pins both halves of this contract).
import './reset.css';
import './theme.css';

export { Editor } from './Editor';
export type { ProtobufProjectData, JsonProjectData, ProjectData } from './Editor';
export { ErrorBoundary } from './ErrorBoundary';
export { renderSvgToString } from './render-common';

// UI Components
export { default as Button } from './components/Button';
export { default as IconButton } from './components/IconButton';
export { default as CircularProgress } from './components/CircularProgress';
export type { CircularProgressProps } from './components/CircularProgress';
export { default as TextField } from './components/TextField';
export { default as SvgIcon } from './components/SvgIcon';
export type { SvgIconProps } from './components/SvgIcon';
export { default as Drawer } from './components/Drawer';
export { default as Snackbar, SnackbarContent } from './components/Snackbar';
export { default as SpeedDial, SpeedDialAction, SpeedDialIcon } from './components/SpeedDial';
export type { CloseReason } from './components/SpeedDial';
export { Tabs, Tab } from './components/Tabs';
export { default as Autocomplete } from './components/Autocomplete';

// New components
export { default as Avatar } from './components/Avatar';
export { default as Paper } from './components/Paper';
export { default as TextLink } from './components/TextLink';
export { default as InputAdornment } from './components/InputAdornment';
export { default as AppBar } from './components/AppBar';
export { default as Toolbar } from './components/Toolbar';
export { default as ImageList, ImageListItem } from './components/ImageList';
export { default as Card, CardContent, CardActions } from './components/Card';
export { default as Checkbox } from './components/Checkbox';
export { default as FormControlLabel } from './components/FormControlLabel';
export { Menu, MenuItem } from './components/Menu';
export { Accordion, AccordionSummary, AccordionDetails } from './components/Accordion';
export { Dialog, DialogTitle, DialogContent, DialogActions, DialogContentText } from './components/Dialog';

// Icons
export * from './components/icons';
