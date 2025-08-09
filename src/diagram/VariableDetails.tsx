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

      // Precise caret mapping via hidden monospace ghost with per-char spans
      const computeOffset = (): number => {
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

          // Find first position whose right edge crosses clickX
          let idx = rights.findIndex((rx) => rx >= clickX);
          if (idx === -1) idx = spans.length; // click beyond end

          // Cleanup
          target.removeChild(ghost);
          return idx;
        } catch {
          // Fallback to proportional mapping
          const len = equationStr.length;
          return Math.max(0, Math.min(len, Math.round((clickX / usableWidth) * len)));
        }
      };

      const offset = computeOffset();

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
