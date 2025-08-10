// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import clsx from 'clsx';
import { styled } from '@mui/material/styles';
import { createEditor, Descendant, Text, Transforms } from 'slate';
import { withHistory } from 'slate-history';
import { Editable, ReactEditor, RenderLeafProps, Slate, withReact } from 'slate-react';
import { Button, Card, CardActions, CardContent, Tab, Tabs, Typography } from '@mui/material';
import katex from 'katex';
import { brewer } from 'chroma-js';

import {
  StockViewElement,
  ViewElement,
  Variable,
  GraphicalFunction,
  GraphicalFunctionScale,
  ApplyToAllEquation,
  ScalarEquation,
  EquationError,
  UnitError,
} from '@system-dynamics/core/datamodel';

import { defined, Series } from '@system-dynamics/core/common';
import { plainDeserialize, plainSerialize } from './drawing/common';
import { CustomElement, FormattedText, CustomEditor } from './drawing/SlateEditor';
import { LookupEditor } from './LookupEditor';
import { errorCodeDescription } from '@system-dynamics/engine';

const SearchbarWidthSm = 359;
const SearchbarWidthMd = 420;
const SearchbarWidthLg = 480;

interface VariableDetailsProps {
  variable: Variable;
  viewElement: ViewElement;
  getLatexEquation?: (ident: string) => string | undefined;
  onDelete: (ident: string) => void;
  onEquationChange: (
    ident: string,
    newEquation: string | undefined,
    newUnits: string | undefined,
    newDoc: string | undefined,
  ) => void;
  onTableChange: (ident: string, newTable: GraphicalFunction | null) => void;
  activeTab: number;
  onActiveTabChange: (newActiveTab: number) => void;
}

// export type VariableDetailsProps = Pick<VariableDetailsPropsFull, 'variable' | 'viewElement' | 'data'>;

interface VariableDetailsState {
  equationContents: Descendant[];
  equationEditor: CustomEditor;
  unitsContents: Descendant[];
  unitsEditor: CustomEditor;
  notesContents: Descendant[];
  notesEditor: CustomEditor;
  editingEquation: boolean;
}

function stringFromDescendants(children: Descendant[]): string {
  return plainSerialize(children);
}

function descendantsFromString(equation: string): CustomElement[] {
  return plainDeserialize('equation', equation);
}

function scalarEquationFor(variable: Variable): string {
  if (variable.equation instanceof ScalarEquation) {
    return variable.equation.equation;
  } else if (variable.equation instanceof ApplyToAllEquation) {
    return '{apply-to-all:}\n' + variable.equation.equation;
  } else {
    return "{ TODO: arrayed variable editing isn't supported yet}";
  }
}

function highlightErrors(
  s: string,
  errors: List<EquationError> | undefined,
  unitErrors: List<UnitError> | undefined,
  isUnits: boolean,
): CustomElement[] {
  const result = descendantsFromString(s);
  if (!isUnits && errors && errors.size > 0) {
    // TODO: multiple errors
    const err = defined(errors.get(0));
    console.log(err);
    // if the end is 0 it means this is a problem we don't have position information for
    if (err.end > 0) {
      const children = defined(result[0]).children as Array<Text>;
      const textChild: string = defined(children[0]).text;

      const beforeText = textChild.substring(0, err.start);
      const errText = textChild.substring(err.start, err.end);
      const afterText = textChild.substring(err.end);

      defined(result[0]).children = [{ text: beforeText }, { text: errText, error: true }, { text: afterText }];
    }
  } else if (unitErrors && unitErrors.size > 0) {
    for (const err of unitErrors) {
      if (isUnits === err.isConsistencyError) {
        continue;
      }
      const children = defined(result[0]).children as Array<Text>;
      const textChild: string = defined(children[0]).text;
      const end = err.end === 0 ? textChild.length : err.end;

      const beforeText = textChild.substring(0, err.start);
      const errText = textChild.substring(err.start, end);
      const afterText = textChild.substring(end);

      const highlighted: FormattedText = isUnits ? { text: errText, error: true } : { text: errText, warning: true };
      defined(result[0]).children = [{ text: beforeText }, highlighted, { text: afterText }];

      break;
    }
  }

  return result;
}

// LaTeX provided by engine (Ast.to_latex); keep trivial passthrough fallback
const passthroughLatex = (s: string) => s;

export const VariableDetails = styled(
  class InnerVariableDetails extends React.PureComponent<
    VariableDetailsProps & { className?: string },
    VariableDetailsState
  > {
    constructor(props: VariableDetailsProps) {
      super(props);

      const { variable } = props;

      const equation = highlightErrors(
        scalarEquationFor(variable),
        props.variable.errors,
        props.variable.unitErrors,
        false,
      );
      const units = highlightErrors(props.variable.units, props.variable.errors, props.variable.unitErrors, true);

      this.state = {
        equationEditor: withHistory(withReact(createEditor())) as unknown as CustomEditor,
        equationContents: equation,
        unitsEditor: withHistory(withReact(createEditor())) as unknown as CustomEditor,
        unitsContents: units,
        notesEditor: withHistory(withReact(createEditor())) as unknown as CustomEditor,
        notesContents: descendantsFromString(props.variable.documentation),
        editingEquation: !!(props.variable.errors && props.variable.errors.size > 0),
      };
    }

    handleEquationChange = (equation: Descendant[]): void => {
      this.setState({ equationContents: equation });
    };

    handleVariableDelete = (): void => {
      this.props.onDelete(defined(this.props.viewElement.ident));
    };

    handleUnitsChange = (equation: Descendant[]): void => {
      this.setState({ unitsContents: equation });
    };

    handleNotesChange = (equation: Descendant[]): void => {
      this.setState({ notesContents: equation });
    };

    handleEquationCancel = (): void => {
      this.setState({
        equationContents: descendantsFromString(scalarEquationFor(this.props.variable)),
        unitsContents: descendantsFromString(this.props.variable.units),
        notesContents: descendantsFromString(this.props.variable.documentation),
        editingEquation: false,
      });
    };

    handleEquationSave = (): void => {
      const { equationContents, unitsContents, notesContents } = this.state;
      const initialEquation = scalarEquationFor(this.props.variable);
      const initialUnits = this.props.variable.units;
      const initialDocs = this.props.variable.documentation;

      const newEquation = stringFromDescendants(equationContents);
      const newUnits = stringFromDescendants(unitsContents);
      const newDocs = stringFromDescendants(notesContents);
      const equation = initialEquation !== newEquation ? newEquation : undefined;
      const units = initialUnits !== newUnits ? newUnits : undefined;
      const docs = initialDocs !== newDocs ? newDocs : undefined;
      if (equation !== undefined || units !== undefined || docs != undefined) {
        this.props.onEquationChange(defined(this.props.viewElement.ident), equation, units, docs);
      }
    };

    formatValue = (value: number | string | Array<number | string>): string => {
      return typeof value === 'number' ? value.toFixed(3) : value.toString();
    };

    // eslint-disable-next-line @typescript-eslint/ban-types
    handleTabChange = (event: React.ChangeEvent<{}>, newValue: number) => {
      this.props.onActiveTabChange(newValue);
    };

    handleAddLookupTable = (): void => {
      const ident = defined(this.props.viewElement.ident);
      const gf = new GraphicalFunction({
        kind: 'continuous',
        xScale: new GraphicalFunctionScale({ min: 0, max: 1 }),
        yScale: new GraphicalFunctionScale({ min: 0, max: 1 }),
        xPoints: undefined,
        yPoints: List([0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0]),
      });
      this.props.onTableChange(ident, gf);
    };

    renderLeaf = (props: RenderLeafProps) => {
      const isError = !!(props.leaf as unknown as any).error;
      const isWarning = !!(props.leaf as unknown as any).warning;
      const errorClass = 'simlin-variabledetails-eqnerror';
      const warningClass = 'simlin-variabledetails-eqnwarning';
      const className = isError ? errorClass : isWarning ? warningClass : undefined;
      return (
        <span {...props.attributes} className={className}>
          {props.children}
        </span>
      );
    };

    renderEquation() {
      const { equationContents } = this.state;
      const initialEquation = scalarEquationFor(this.props.variable);
      const initialUnits = this.props.variable.units;
      const initialDocs = this.props.variable.documentation;

      const data: Readonly<Array<Series>> | undefined = this.props.variable.data;

      const lines = [];

      let yMin = 0;
      let yMax = 0;
      const series: Array<any> = [];
      if (data) {
        let i = 0;
        const colors = brewer.Dark2;
        for (const dataset of data) {
          const name = data.length === 1 ? 'y' : dataset.name;
          for (let i = 0; data && i < dataset.time.length; i++) {
            const x = dataset.time[i];
            const y = dataset.values[i];
            const point: any = { x };
            point[name] = y;
            series.push(point);
            if (y < yMin) {
              yMin = y;
            }
            if (y > yMax) {
              yMax = y;
            }
          }
          const colorOff = i % colors.length;
          lines.push(
            <Line
              key={name}
              yAxisId="1"
              type="linear"
              dataKey={name}
              stroke={colors[colorOff]}
              animationDuration={300}
              dot={false}
            />,
          );
          i++;
        }
      }

      yMin = Math.floor(yMin);
      yMax = Math.ceil(yMax);

      const charWidth = Math.max(yMin.toFixed(0).length, yMax.toFixed(0).length);
      const yAxisWidth = Math.max(40, 20 + charWidth * 6);

      // enable saving and canceling if the equation has changed
      const equationActionsEnabled =
        initialEquation !== stringFromDescendants(equationContents) ||
        initialUnits !== stringFromDescendants(this.state.unitsContents) ||
        initialDocs !== stringFromDescendants(this.state.notesContents);

      const { left, right } = {
        left: 'dataMin',
        right: 'dataMax',
      };

      let chartOrErrors;
      const errors = this.props.variable.errors;
      const unitErrors = this.props.variable.unitErrors;
      if (errors || unitErrors) {
        const errorList: Array<React.ReactElement> = [];
        if (errors) {
          errors.forEach((error) => {
            errorList.push(
              <Typography className="simlin-variabledetails-errorlist">
                error: {errorCodeDescription(error.code)}
              </Typography>,
            );
          });
        }
        if (unitErrors) {
          unitErrors.forEach((error) => {
            const details = error.details;
            errorList.push(
              <Typography className="simlin-variabledetails-errorlist">
                unit error: {errorCodeDescription(error.code)}
                {details ? `: ${details}` : undefined}
              </Typography>,
            );
          });
        }
        chartOrErrors = errorList;
      } else {
        chartOrErrors = (
          <ResponsiveContainer width="100%" height={300}>
            <LineChart data={series}>
              <CartesianGrid horizontal={true} vertical={false} />
              <XAxis allowDataOverflow={true} dataKey="x" domain={[left, right]} type="number" />
              <YAxis
                width={yAxisWidth}
                allowDataOverflow={true}
                domain={[yMin, yMax]}
                type="number"
                dataKey="y"
                yAxisId="1"
              />
              <Tooltip formatter={this.formatValue} />
              {lines}
            </LineChart>
          </ResponsiveContainer>
        );
      }

      const showPreview = !errors && !unitErrors && !this.state.editingEquation;

      const equationStr = stringFromDescendants(equationContents);
      let latexHTML = '';
      if (showPreview) {
        try {
          const ident = defined(this.props.viewElement.ident);
          let latex = this.props.getLatexEquation
            ? (this.props.getLatexEquation(ident) ?? passthroughLatex(equationStr))
            : passthroughLatex(equationStr);
          // Hint line breaks after common binary operators and commas for nicer wrapping
          const insertBreaks = (s: string): string =>
            s
              .replace(/\\cdot/g, '\\cdot\\allowbreak{} ')
              .replace(/\\times/g, '\\times\\allowbreak{} ')
              .replace(/\+/g, '+\\allowbreak{} ')
              .replace(/-/g, '-\\allowbreak{} ')
              .replace(/=/g, '=\\allowbreak{} ')
              .replace(/,/g, ',\\allowbreak{} ');
          latex = insertBreaks(latex);
          latexHTML = katex.renderToString(latex, { throwOnError: false, displayMode: true });
        } catch (e) {
          // fall back to plain text
          latexHTML = '';
        }
      }

      return (
        <CardContent>
          {showPreview ? (
            <div
              className="simlin-variabledetails-eqnpreview"
              onClick={(e) => this.handlePreviewClick(e, equationStr)}
              // eslint-disable-next-line react/no-danger
              dangerouslySetInnerHTML={{ __html: latexHTML }}
            />
          ) : (
            <Slate
              editor={this.state.equationEditor}
              initialValue={this.state.equationContents}
              onChange={this.handleEquationChange}
            >
              <Editable
                className="simlin-variabledetails-eqneditor"
                renderLeaf={this.renderLeaf}
                placeholder="Enter an equation..."
                spellCheck={false}
                autoFocus
                onBlur={() => {
                  this.handleEquationSave();
                  if (!this.props.variable.errors && !this.props.variable.unitErrors) {
                    this.setState({ editingEquation: false });
                  }
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Escape') {
                    this.setState({ editingEquation: false });
                  }
                }}
              />
            </Slate>
          )}

          <Slate
            editor={this.state.unitsEditor}
            initialValue={this.state.unitsContents}
            onChange={this.handleUnitsChange}
          >
            <Editable
              className="simlin-variabledetails-unitseditor"
              renderLeaf={this.renderLeaf}
              placeholder="Enter units..."
              spellCheck={false}
              onBlur={this.handleEquationSave}
            />
          </Slate>

          <Slate
            editor={this.state.notesEditor}
            initialValue={this.state.notesContents}
            onChange={this.handleNotesChange}
          >
            <Editable
              className="simlin-variabledetails-noteseditor"
              renderLeaf={this.renderLeaf}
              placeholder="Documentation"
              spellCheck={false}
              onBlur={this.handleEquationSave}
            />
          </Slate>

          <CardActions>
            <Button
              size="small"
              color="secondary"
              onClick={this.handleVariableDelete}
              className="simlin-variabledetails-buttonleft"
            >
              Delete
            </Button>
            <div className="simlin-variabledetails-buttonright">
              <Button
                size="small"
                color="primary"
                disabled={!equationActionsEnabled}
                onClick={this.handleEquationCancel}
              >
                Cancel
              </Button>
              <Button size="small" color="primary" disabled={!equationActionsEnabled} onClick={this.handleEquationSave}>
                Save
              </Button>
            </div>
          </CardActions>

          <hr />
          <br />
          {chartOrErrors}
        </CardContent>
      );
    }

    handlePreviewClick = (e: React.MouseEvent<HTMLDivElement>, equationStr: string) => {
      const target = e.currentTarget as HTMLElement;
      const rect = target.getBoundingClientRect();
      const style = window.getComputedStyle(target);
      const padLeft = parseFloat(style.paddingLeft || '0');
      const padRight = parseFloat(style.paddingRight || '0');
      const usableWidth = Math.max(1, rect.width - padLeft - padRight);
      const clickX = Math.max(0, Math.min(usableWidth, e.clientX - rect.left - padLeft));

      // Map click to text offset.
      // Strategy:
      // 1) Try to resolve the specific KaTeX glyph under the click, map it to the matching ASCII
      //    character, and place the caret immediately after the closest matching occurrence in the
      //    ASCII equation (by proximity to the coarse proportional index).
      // 2) Otherwise, prefer proportional mapping over KaTeX content width.
      // 3) Fall back to a hidden monospace ghost with per-char spans and midpoints.
      // 4) Finally fall back to plain proportional mapping over the container width.
      const computeOffset = (): number => {
        // Utility: map KaTeX glyph to ASCII char
        const mapGlyphToAscii = (ch: string): string => {
          if (ch === '·' || ch === '×' || ch === '⋅') return '*';
          if (ch === '−') return '-';
          return ch;
        };

        // Build a linear list of KaTeX glyphs with positions to enable context disambiguation
        const buildGlyphs = () => {
          const list: { raw: string; ascii: string; left: number; right: number; top: number; bottom: number }[] = [];
          const root = target.querySelector('.katex') as HTMLElement | null;
          if (!root) return { list, contentRect: target.getBoundingClientRect() };
          const html = target.querySelector('.katex .katex-html, .katex-display .katex-html') as HTMLElement | null;
          const contentRect = (html ?? root).getBoundingClientRect();
          try {
            const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
            while (walker.nextNode()) {
              const tn = walker.currentNode as any;
              const text: string = (tn && (tn as any).nodeValue) || '';
              for (let i = 0; i < text.length; i++) {
                const rng = document.createRange();
                rng.setStart(tn as Node, i);
                rng.setEnd(tn as Node, i + 1);
                const r = rng.getBoundingClientRect();
                if (!r || r.width <= 0 || r.height <= 0) continue;
                const raw = text[i];
                const ascii = mapGlyphToAscii(raw);
                list.push({ raw, ascii, left: r.left, right: r.right, top: r.top, bottom: r.bottom });
              }
            }
          } catch {
            // ignore
          }
          try {
            // eslint-disable-next-line no-console
            console.log('[CaretMap] glyphs', {
              count: list.length,
              ascii: list.map((g) => g.ascii).join(''),
              first20: list
                .slice(0, 20)
                .map((g) => g.ascii)
                .join(''),
            });
          } catch {}
          return { list, contentRect };
        };

        // Utility: try to find the KaTeX glyph index at click coordinates
        const glyphIndexAtPoint = (glyphs: ReturnType<typeof buildGlyphs>['list']): number => {
          // First, exact hit
          for (let i = 0; i < glyphs.length; i++) {
            const g = glyphs[i];
            if (e.clientX >= g.left && e.clientX <= g.right && e.clientY >= g.top && e.clientY <= g.bottom) {
              try {
                // eslint-disable-next-line no-console
                console.log('[CaretMap] exact glyph hit', { index: i, ascii: g.ascii, raw: g.raw });
              } catch {}
              return i;
            }
          }
          // Otherwise nearest on same line (by center distance)
          let best = -1;
          let bestDist = Number.POSITIVE_INFINITY;
          for (let i = 0; i < glyphs.length; i++) {
            const g = glyphs[i];
            const cx = (g.left + g.right) / 2;
            const cy = (g.top + g.bottom) / 2;
            const dx = Math.abs(e.clientX - cx);
            const dy = Math.abs(e.clientY - cy);
            const d = dx + dy * 0.5;
            if (d < bestDist) {
              bestDist = d;
              best = i;
            }
          }
          try {
            if (best >= 0) {
              const g = glyphs[best];
              // eslint-disable-next-line no-console
              console.log('[CaretMap] nearest glyph', { index: best, ascii: g.ascii, raw: g.raw, dist: bestDist });
            } else {
              // eslint-disable-next-line no-console
              console.log('[CaretMap] no glyph found');
            }
          } catch {}
          return best;
        };

        // Try glyph-based mapping first
        try {
          const { list: glyphs, contentRect } = buildGlyphs();
          if (glyphs.length > 0) {
            const gidx = glyphIndexAtPoint(glyphs);
            if (gidx >= 0) {
              const center = glyphs[gidx].ascii;
              const prev = gidx > 0 ? glyphs[gidx - 1].ascii : '';
              const next = gidx + 1 < glyphs.length ? glyphs[gidx + 1].ascii : '';
              const pattern = (prev + center + next).trim();
              const len = equationStr.length;
              // Try word-token based mapping for alphanumeric/underscore sequences
              const isTokenChar = (ch: string) => /[A-Za-z0-9_]/.test(ch);
              if (isTokenChar(center)) {
                let li = gidx;
                let ri = gidx;
                while (li - 1 >= 0 && isTokenChar(glyphs[li - 1].ascii)) li--;
                while (ri + 1 < glyphs.length && isTokenChar(glyphs[ri + 1].ascii)) ri++;
                const token = glyphs
                  .slice(li, ri + 1)
                  .map((g) => g.ascii)
                  .join('');
                const posInToken = gidx - li;
                if (token.length > 1) {
                  // Prefer nearest occurrence of the token to the coarse proportional index
                  const cx = Math.max(0, Math.min(contentRect.width, e.clientX - contentRect.left));
                  const coarse = Math.max(0, Math.min(len, Math.round((cx / Math.max(1, contentRect.width)) * len)));
                  const candidates: number[] = [];
                  let s = 0;
                  for (;;) {
                    const idx = equationStr.indexOf(token, s);
                    if (idx === -1) break;
                    candidates.push(idx);
                    s = idx + 1;
                  }
                  if (candidates.length > 0) {
                    let choiceIdx = candidates[0];
                    let bestDist = Math.abs(choiceIdx - coarse);
                    for (const c of candidates) {
                      const d = Math.abs(c - coarse);
                      if (d < bestDist) {
                        bestDist = d;
                        choiceIdx = c;
                      }
                    }
                    try {
                      // eslint-disable-next-line no-console
                      console.log('[CaretMap] token match', { token, candidates, choiceIdx, posInToken });
                    } catch {}
                    return choiceIdx + posInToken + 1;
                  } else {
                    try {
                      // eslint-disable-next-line no-console
                      console.log('[CaretMap] token not found', { token });
                    } catch {}
                  }
                }
              }
              // Signature neighbor matching for punctuation/operators/digits (and single-char tokens)
              const isSigChar = (ch: string) => /[A-Za-z0-9_()*+\-]/.test(ch);
              const prevSig = (() => {
                for (let k = gidx - 1; k >= 0; k--) if (isSigChar(glyphs[k].ascii)) return glyphs[k].ascii; return '';
              })();
              const nextSig = (() => {
                for (let k = gidx + 1; k < glyphs.length; k++) if (isSigChar(glyphs[k].ascii)) return glyphs[k].ascii; return '';
              })();
              if (center && (prevSig || nextSig)) {
                const positions: number[] = [];
                for (let i = 0; i < len; i++) if (equationStr[i] === center) positions.push(i);
                const matchScore = (idx: number) => {
                  let score = 0;
                  if (prevSig) {
                    let j = idx - 1; while (j >= 0 && /\s/.test(equationStr[j])) j--; if (j >= 0 && equationStr[j] === prevSig) score += 2;
                  }
                  if (nextSig) {
                    let j = idx + 1; while (j < len && /\s/.test(equationStr[j])) j++; if (j < len && equationStr[j] === nextSig) score += 2;
                  }
                  return score;
                };
                let bestIdx = -1;
                let bestScore = -1;
                for (const p of positions) { const s = matchScore(p); if (s > bestScore) { bestScore = s; bestIdx = p; } }
                if (bestIdx >= 0) {
                  try { console.log('[CaretMap] sig-neighbor match', { center, prevSig, nextSig, bestIdx, bestScore }); } catch {}
                  return bestIdx + 1;
                }
                try { console.log('[CaretMap] sig-neighbor no match', { center, prevSig, nextSig, positions }); } catch {}
              }

              // Try contextual match: prev+center+next
              if (pattern.length >= 2) {
                const idx = equationStr.indexOf(pattern);
                if (idx >= 0) {
                  try {
                    // eslint-disable-next-line no-console
                    console.log('[CaretMap] context match', { pattern, idx, prev, center, next });
                  } catch {}
                  const centerOffset = prev ? 1 : 0;
                  return idx + centerOffset + 1;
                }
                try {
                  // eslint-disable-next-line no-console
                  console.log('[CaretMap] context not found', { pattern });
                } catch {}
              }
              // Fallback: match just center using nth occurrence by glyph index
              if (center) {
                // Count occurrences among glyphs up to this glyph to get the per-char occurrence index
                let glyphOcc = 0;
                for (let k = 0; k < gidx; k++) if (glyphs[k].ascii === center) glyphOcc++;
                let count = -1;
                for (let i = 0; i < len; i++) {
                  if (equationStr[i] === center) {
                    count++;
                    if (count === glyphOcc) {
                      try {
                        // eslint-disable-next-line no-console
                        console.log('[CaretMap] nth occurrence fallback', { center, glyphOcc, index: i });
                      } catch {}
                      return i + 1;
                    }
                  }
                }
                try {
                  // eslint-disable-next-line no-console
                  console.log('[CaretMap] nth occurrence not found', { center, glyphOcc });
                } catch {}
              }
              // Fallback: choose occurrence closest to coarse proportional position
              const cx = Math.max(0, Math.min(contentRect.width, e.clientX - contentRect.left));
              const coarse = Math.max(0, Math.min(len, Math.round((cx / Math.max(1, contentRect.width)) * len)));
              const positions: number[] = [];
              for (let i = 0; i < len; i++) if (equationStr[i] === center) positions.push(i);
              if (positions.length > 0) {
                let choice = positions[0];
                let bestDist = Math.abs(choice - coarse);
                for (const p of positions) {
                  const d = Math.abs(p - coarse);
                  if (d < bestDist) {
                    bestDist = d;
                    choice = p;
                  }
                }
                try {
                  // eslint-disable-next-line no-console
                  console.log('[CaretMap] coarse fallback', { center, coarse, positions, choice });
                } catch {}
                return choice + 1;
              }
              try {
                // eslint-disable-next-line no-console
                console.log('[CaretMap] no positions for center', { center });
              } catch {}
            }
          }
        } catch {
          // ignore and try next strategy
        }

        try {
          // Prefer mapping relative to actual KaTeX content box, ignoring padding.
          const katexHtml = target.querySelector(
            '.katex .katex-html, .katex-display .katex-html',
          ) as HTMLElement | null;
          if (katexHtml) {
            const contentRect = katexHtml.getBoundingClientRect();
            const cx = Math.max(0, Math.min(contentRect.width, e.clientX - contentRect.left));
            const len = equationStr.length;
            try {
              // eslint-disable-next-line no-console
              console.log('[CaretMap] proportional over KaTeX content', { width: contentRect.width, cx, len });
            } catch {}
            return Math.max(0, Math.min(len, Math.round((cx / Math.max(1, contentRect.width)) * len)));
          }
        } catch {
          // ignore and try ghost mapping
        }

        // Precise caret mapping via hidden monospace ghost with per-char spans
        try {
          const ghost = document.createElement('div');
          ghost.style.position = 'absolute';
          ghost.style.left = `${padLeft}px`;
          ghost.style.top = '0';
          ghost.style.visibility = 'hidden';
          ghost.style.whiteSpace = 'pre-wrap';
          ghost.style.pointerEvents = 'none';
          ghost.style.width = `${usableWidth}px`;
          // Match editor font
          ghost.style.fontFamily = "'Roboto Mono', monospace";
          // Try to inherit approximate size
          ghost.style.fontSize = window.getComputedStyle(document.body).fontSize || '16px';
          target.appendChild(ghost);

          const spans: HTMLSpanElement[] = [];
          for (let i = 0; i < equationStr.length; i++) {
            const s = document.createElement('span');
            s.textContent = equationStr[i];
            // Ensure each char is measurable
            s.style.display = 'inline-block';
            ghost.appendChild(s);
            spans.push(s);
          }

          // Build cumulative right edge positions
          const ghostRect = ghost.getBoundingClientRect();
          const rights: number[] = [];
          for (let i = 0; i < spans.length; i++) {
            const r = spans[i].getBoundingClientRect();
            rights.push(r.right - ghostRect.left);
          }

          // Prefer nearest character boundary using midpoints between glyph boxes
          // left edge for i is rights[i-1] (or 0 for i=0)
          let idx = spans.length;
          for (let i = 0; i < spans.length; i++) {
            const left = i === 0 ? 0 : rights[i - 1];
            const right = rights[i];
            const mid = (left + right) / 2;
            if (clickX < mid) {
              idx = i;
              break;
            }
            // If past the midpoint, caret should go after this character; keep scanning
          }

          // Cleanup
          target.removeChild(ghost);
          try {
            // eslint-disable-next-line no-console
            console.log('[CaretMap] ghost midpoint idx', { idx });
          } catch {}
          return idx;
        } catch {
          // Fallback to proportional mapping
          const len = equationStr.length;
          try {
            // eslint-disable-next-line no-console
            console.log('[CaretMap] container proportional fallback', { usableWidth, clickX, len });
          } catch {}
          return Math.max(0, Math.min(len, Math.round((clickX / usableWidth) * len)));
        }
      };

      const offset = computeOffset();
      try {
        // eslint-disable-next-line no-console
        console.log('[CaretMap] final offset', { offset });
      } catch {}

      this.setState({ editingEquation: true }, () => {
        // Focus and set caret after the editor renders
        requestAnimationFrame(() => {
          try {
            const editor = this.state.equationEditor;
            ReactEditor.focus(editor);
            Transforms.select(editor, {
              anchor: { path: [0, 0], offset },
              focus: { path: [0, 0], offset },
            });
          } catch {
            // ignore if selection fails; user can click to place caret
          }
        });
      });
    };

    handleLookupChange = (ident: string, newTable: GraphicalFunction | null) => {
      this.props.onTableChange(ident, newTable);
    };

    renderLookup() {
      const { variable } = this.props;

      let table;
      if (variable.gf) {
        table = <LookupEditor variable={variable} onLookupChange={this.handleLookupChange} />;
      } else {
        table = (
          <CardContent>
            <Button
              variant="contained"
              color="secondary"
              onClick={this.handleAddLookupTable}
              className="simlin-variabledetails-addlookupbutton"
            >
              Add lookup table
            </Button>
            <br />
            <i>
              A lookup table is a non-linear function indexed by the variable{"'"}s equation. You edit the function by
              dragging your mouse or finger across the graph.
            </i>
          </CardContent>
        );
      }

      return table;
    }

    render() {
      const { activeTab, className, viewElement } = this.props;

      const equationType = viewElement instanceof StockViewElement ? 'Initial Value' : 'Equation';
      const content = activeTab === 0 ? this.renderEquation() : this.renderLookup();
      const lookupTab = viewElement instanceof StockViewElement ? undefined : <Tab label="Lookup Function" />;

      return (
        <Card className={clsx(className, 'simlin-variabledetails-card')} elevation={1}>
          <Tabs
            className="simlin-variabledetails-inner"
            variant="fullWidth"
            value={activeTab}
            indicatorColor="primary"
            textColor="primary"
            onChange={this.handleTabChange}
            aria-label="Equation details selector"
          >
            <Tab label={equationType} />
            {lookupTab}
          </Tabs>

          {content}
        </Card>
      );
    }
  },
)(({ theme }) => ({
  '&.simlin-variabledetails-card': {
    [theme.breakpoints.up('lg')]: {
      width: SearchbarWidthLg,
    },
    [theme.breakpoints.between('md', 'lg')]: {
      width: SearchbarWidthMd,
    },
    [theme.breakpoints.down('md')]: {
      width: SearchbarWidthSm,
    },
  },
  '.simlin-variabledetails-inner': {
    paddingTop: 52,
  },
  '.simlin-variabledetails-eqneditor': {
    backgroundColor: 'rgba(245, 245, 245)',
    borderRadius: 4,
    marginTop: 4,
    padding: 4,
    height: 80,
    fontFamily: "'Roboto Mono', monospace",
    overflowY: 'auto',
  },
  '.simlin-variabledetails-eqnpreview': {
    backgroundColor: 'rgba(245, 245, 245)',
    borderRadius: 4,
    marginTop: 4,
    padding: 8,
    minHeight: 80, // match editor height to avoid layout shift
    cursor: 'text',
    transition: 'opacity 120ms ease-in-out',
    display: 'flex',
    alignItems: 'center',
    // normalize KaTeX display mode margins and size
    '& .katex-display': {
      margin: '0 !important',
      textAlign: 'left',
      width: '100%',
    },
    '& .katex': {
      fontSize: '1.1em',
      whiteSpace: 'normal',
      overflowWrap: 'anywhere',
      wordBreak: 'break-word',
      maxWidth: '100%',
    },
  },
  '.simlin-variabledetails-unitseditor': {
    backgroundColor: 'rgba(245, 245, 245)',
    borderRadius: 4,
    marginTop: 4,
    padding: 4,
    height: 36,
    fontFamily: "'Roboto Mono', monospace",
    overflowY: 'auto',
  },
  '.simlin-variabledetails-noteseditor': {
    backgroundColor: 'rgba(245, 245, 245)',
    borderRadius: 4,
    marginTop: 4,
    padding: 4,
    height: 56,
    fontFamily: "'Roboto Mono', monospace",
    overflowY: 'auto',
  },
  '.simlin-variabledetails-eqnerror': {
    textDecoration: 'underline wavy red',
  },
  '.simlin-variabledetails-eqnwarning': {
    textDecoration: 'underline wavy orange',
  },
  '.simlin-variabledetails-buttonleft': {
    float: 'left',
    marginRight: 'auto',
  },
  '.simlin-variabledetails-buttonright': {
    float: 'right',
  },
  '.simlin-variabledetails-addlookupbutton': {
    display: 'block',
    marginLeft: 'auto',
    marginRight: 'auto',
  },
  '.simlin-variabledetails-errorlist': {
    color: '#cc0000',
  },
}));
