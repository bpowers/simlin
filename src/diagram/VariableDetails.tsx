// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';

import { createEditor, Descendant, Text } from 'slate';
import { withHistory } from 'slate-history';
import { Editable, ReactEditor, RenderLeafProps, Slate, withReact } from 'slate-react';

import { Button, Card, CardActions, CardContent, Tab, Tabs, Typography } from '@material-ui/core';
import { createStyles, withStyles, WithStyles, Theme } from '@material-ui/core/styles';

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
import { EquationElement, FormattedText } from './drawing/SlateEditor';
import { LookupEditor } from './LookupEditor';
import { errorCodeDescription } from '@system-dynamics/engine';

const SearchbarWidthSm = 359;
const SearchbarWidthMd = 420;
const SearchbarWidthLg = 480;

const styles = ({ breakpoints }: Theme) =>
  createStyles({
    card: {
      [breakpoints.up('lg')]: {
        width: SearchbarWidthLg,
      },
      [breakpoints.between('md', 'lg')]: {
        width: SearchbarWidthMd,
      },
      [breakpoints.down('md')]: {
        width: SearchbarWidthSm,
      },
    },
    cardInner: {
      paddingTop: 52,
    },
    editorActions: {},
    eqnEditor: {
      backgroundColor: 'rgba(245, 245, 245)',
      borderRadius: 4,
      marginTop: 4,
      padding: 4,
      height: 80,
      fontFamily: "'Roboto Mono', monospace",
      overflowY: 'auto',
    },
    unitsEditor: {
      backgroundColor: 'rgba(245, 245, 245)',
      borderRadius: 4,
      marginTop: 4,
      padding: 4,
      height: 36,
      fontFamily: "'Roboto Mono', monospace",
      overflowY: 'auto',
    },
    notesEditor: {
      backgroundColor: 'rgba(245, 245, 245)',
      borderRadius: 4,
      marginTop: 4,
      padding: 4,
      height: 56,
      fontFamily: "'Roboto Mono', monospace",
      overflowY: 'auto',
    },
    eqnError: {
      textDecoration: 'underline wavy red',
    },
    eqnWarning: {
      textDecoration: 'underline wavy orange',
    },
    buttonLeft: {
      float: 'left',
      marginRight: 'auto',
    },
    buttonRight: {
      float: 'right',
    },
    addLookupButton: {
      display: 'block',
      marginLeft: 'auto',
      marginRight: 'auto',
    },
    errorList: {
      color: '#cc0000',
    },
  });

interface VariableDetailsPropsFull extends WithStyles<typeof styles> {
  variable: Variable;
  viewElement: ViewElement;
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
  equationEditor: ReactEditor;
  unitsContents: Descendant[];
  unitsEditor: ReactEditor;
  notesContents: Descendant[];
  notesEditor: ReactEditor;
}

function stringFromDescendants(children: Descendant[]): string {
  return plainSerialize(children);
}

function descendantsFromString(equation: string): EquationElement[] {
  return [
    {
      type: 'equation',
      children: plainDeserialize(equation),
    },
  ];
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
): EquationElement[] {
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

export const VariableDetails = withStyles(styles)(
  class InnerVariableDetails extends React.PureComponent<VariableDetailsPropsFull, VariableDetailsState> {
    constructor(props: VariableDetailsPropsFull) {
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
        equationEditor: withHistory(withReact(createEditor())),
        equationContents: equation,
        unitsEditor: withHistory(withReact(createEditor())),
        unitsContents: units,
        notesEditor: withHistory(withReact(createEditor())),
        notesContents: descendantsFromString(props.variable.documentation),
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

    formatValue = (value: string | number | (string | number)[]): string | (string | number)[] => {
      return typeof value === 'number' ? value.toFixed(3) : value;
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
      const isError = !!((props.leaf as unknown) as any).error;
      const isWarning = !!((props.leaf as unknown) as any).warning;
      const errorClass = this.props.classes.eqnError;
      const warningClass = this.props.classes.eqnWarning;
      const className = isError ? errorClass : isWarning ? warningClass : undefined;
      return (
        <span {...props.attributes} className={className}>
          {props.children}
        </span>
      );
    };

    renderEquation() {
      const { classes } = this.props;
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
              <Typography className={classes.errorList}>error: {errorCodeDescription(error.code)}</Typography>,
            );
          });
        }
        if (unitErrors) {
          unitErrors.forEach((error) => {
            const details = error.details;
            errorList.push(
              <Typography className={classes.errorList}>
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

      return (
        <CardContent>
          <Slate
            editor={this.state.equationEditor}
            value={this.state.equationContents}
            onChange={this.handleEquationChange}
          >
            <Editable
              className={classes.eqnEditor}
              renderLeaf={this.renderLeaf}
              placeholder="Enter an equation..."
              spellCheck={false}
              onBlur={this.handleEquationSave}
            />
          </Slate>

          <Slate editor={this.state.unitsEditor} value={this.state.unitsContents} onChange={this.handleUnitsChange}>
            <Editable
              className={classes.unitsEditor}
              renderLeaf={this.renderLeaf}
              placeholder="Enter units..."
              spellCheck={false}
              onBlur={this.handleEquationSave}
            />
          </Slate>

          <Slate editor={this.state.notesEditor} value={this.state.notesContents} onChange={this.handleNotesChange}>
            <Editable
              className={classes.notesEditor}
              renderLeaf={this.renderLeaf}
              placeholder="Documentation"
              spellCheck={false}
              onBlur={this.handleEquationSave}
            />
          </Slate>

          <CardActions className={classes.editorActions}>
            <Button size="small" color="secondary" onClick={this.handleVariableDelete} className={classes.buttonLeft}>
              Delete
            </Button>
            <div className={classes.buttonRight}>
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

    handleLookupChange = (ident: string, newTable: GraphicalFunction | null) => {
      this.props.onTableChange(ident, newTable);
    };

    renderLookup() {
      const { classes, variable } = this.props;

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
              className={classes.addLookupButton}
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
      const { activeTab, classes, viewElement } = this.props;

      const equationType = viewElement instanceof StockViewElement ? 'Initial Value' : 'Equation';
      const content = activeTab === 0 ? this.renderEquation() : this.renderLookup();
      const lookupTab = viewElement instanceof StockViewElement ? undefined : <Tab label="Lookup Function" />;

      return (
        <Card className={classes.card} elevation={1}>
          <Tabs
            className={classes.cardInner}
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
);
