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

export const CheckIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M9 16.17 4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z" />
  </SvgIcon>
);

export const AccountCircleIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2m0 4c1.93 0 3.5 1.57 3.5 3.5S13.93 13 12 13s-3.5-1.57-3.5-3.5S10.07 6 12 6m0 14c-2.03 0-4.43-.82-6.14-2.88C7.55 15.8 9.68 15 12 15s4.45.8 6.14 2.12C16.43 19.18 14.03 20 12 20" />
  </SvgIcon>
);

export const ExpandMoreIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M16.59 8.59 12 13.17 7.41 8.59 6 10l6 6 6-6z" />
  </SvgIcon>
);

export const AppleIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M17.05 20.28c-.98.95-2.05.8-3.08.35-1.09-.46-2.09-.48-3.24 0-1.44.62-2.2.44-3.06-.35C2.79 15.25 3.51 7.59 9.05 7.31c1.35.07 2.29.74 3.08.8 1.18-.24 2.31-.93 3.57-.84 1.51.12 2.65.72 3.4 1.8-3.12 1.87-2.38 5.98.48 7.13-.57 1.5-1.31 2.99-2.54 4.09zM12.03 7.25c-.15-2.23 1.66-4.07 3.74-4.25.29 2.58-2.34 4.5-3.74 4.25" />
  </SvgIcon>
);

export const EmailIcon = (props: IconProps) => (
  <SvgIcon {...props}>
    <path d="M20 4H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2m0 4-8 5-8-5V6l8 5 8-5z" />
  </SvgIcon>
);
