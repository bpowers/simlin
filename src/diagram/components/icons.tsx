// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import SvgIcon, { type SvgIconProps } from './SvgIcon';

type IconProps = SvgIconProps;

export const ClearIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z" />
  </SvgIcon>
);

export const CloseIcon = ClearIcon;

export const EditIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M3 17.25V21h3.75L17.81 9.94l-3.75-3.75zM20.71 7.04c.39-.39.39-1.02 0-1.41l-2.34-2.34a.9959.9959 0 0 0-1.41 0l-1.83 1.83 3.75 3.75z" />
  </SvgIcon>
);

export const MenuIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M3 18h18v-2H3zm0-5h18v-2H3zm0-7v2h18V6z" />
  </SvgIcon>
);

export const ArrowBackIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20z" />
  </SvgIcon>
);

export const CloudDownloadIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96M17 13l-5 5-5-5h3V9h4v4z" />
  </SvgIcon>
);

export const RedoIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M18.4 10.6C16.55 8.99 14.15 8 11.5 8c-4.65 0-8.58 3.03-9.96 7.22L3.9 16c1.05-3.19 4.05-5.5 7.6-5.5 1.95 0 3.73.72 5.12 1.88L13 16h9V7z" />
  </SvgIcon>
);

export const UndoIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M12.5 8c-2.65 0-5.05.99-6.9 2.6L2 7v9h9l-3.62-3.62c1.39-1.16 3.16-1.88 5.12-1.88 3.54 0 6.55 2.31 7.6 5.5l2.37-.78C21.08 11.03 17.15 8 12.5 8" />
  </SvgIcon>
);

export const AddIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6z" />
  </SvgIcon>
);

export const RemoveIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M19 13H5v-2h14z" />
  </SvgIcon>
);

export const PhotoCameraIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M9 2 7.17 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2h-3.17L15 2zm3 15c-2.76 0-5-2.24-5-5s2.24-5 5-5 5 2.24 5 5-2.24 5-5 5" />
    <circle cx="12" cy="12" r="3.2" />
  </SvgIcon>
);

export const CheckCircleIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2m-2 15-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8z" />
  </SvgIcon>
);

export const ErrorIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2m1 15h-2v-2h2zm0-4h-2V7h2z" />
  </SvgIcon>
);

export const InfoIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2m1 15h-2v-6h2zm0-8h-2V7h2z" />
  </SvgIcon>
);

export const WarningIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M1 21h22L12 2zm12-3h-2v-2h2zm0-4h-2v-4h2z" />
  </SvgIcon>
);
