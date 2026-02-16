// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { BaseEditor } from 'slate';
import { ReactEditor } from 'slate-react';
import { HistoryEditor } from 'slate-history';

export type CustomEditor = BaseEditor & ReactEditor & HistoryEditor;

export type LabelElement = {
  type: 'label';
  children: CustomText[];
};

export type EquationElement = {
  type: 'equation';
  children: CustomText[];
};

export type CustomElement = LabelElement | EquationElement;

export type FormattedText = { text: string; error?: true; warning?: true };

export type CustomText = FormattedText;

declare module 'slate' {
  interface CustomTypes {
    Editor: CustomEditor;
    Element: CustomElement;
    Text: CustomText;
  }
}
