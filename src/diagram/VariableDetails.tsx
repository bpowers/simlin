// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';

import { createEditor, Node, Text } from 'slate';
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
} from '@system-dynamics/core/datamodel';

import { defined, Series } from '@system-dynamics/core/common';
import { plainDeserialize, plainSerialize } from './drawing/common';
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
    },
    eqnError: {
      textDecoration: 'underline wavy red',
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
  onEquationChange: (ident: string, newEquation: string) => void;
  onTableChange: (ident: string, newTable: GraphicalFunction | null) => void;
  activeTab: number;
  onActiveTabChange: (newActiveTab: number) => void;
}

// export type VariableDetailsProps = Pick<VariableDetailsPropsFull, 'variable' | 'viewElement' | 'data'>;

interface VariableDetailsState {
  equation: Node[];
  editor: ReactEditor;
}

function equationFromValue(children: Node[]): string {
  return plainSerialize(children);
}

function valueFromEquation(equation: string): Node[] {
  return plainDeserialize(equation);
}

function scalarEquationFor(variable: Variable): string {
  if (variable.equation instanceof ScalarEquation) {
    return variable.equation.equation;
  } else if (variable.equation instanceof ApplyToAllEquation) {
    return '{apply-to-all:}\n' + variable.equation.equation;
  } else {
    return "{ TODO: arrayed variables aren't supported yet}";
  }
}

export const VariableDetails = withStyles(styles)(
  class InnerVariableDetails extends React.PureComponent<VariableDetailsPropsFull, VariableDetailsState> {
    readonly lookupRef: React.RefObject<typeof LineChart>;

    constructor(props: VariableDetailsPropsFull) {
      super(props);

      const { variable } = props;
      this.lookupRef = React.createRef();

      const equation = valueFromEquation(scalarEquationFor(variable));
      const errors = props.variable.errors;
      if (errors) {
        // TODO: multiple errors
        const err = defined(errors.get(0));
        // if the end is 0 it means this is a problem we don't have position information for
        if (err.end > 0) {
          const children = defined(equation[0]).children as Array<Text>;
          const textChild: string = defined(children[0]).text;

          const beforeText = textChild.substring(0, err.start);
          const errText = textChild.substring(err.start, err.end);
          const afterText = textChild.substring(err.end);

          defined(equation[0]).children = [{ text: beforeText }, { text: errText, error: true }, { text: afterText }];
        }
      }

      this.state = {
        editor: withHistory(withReact(createEditor())),
        equation,
      };
    }

    handleEquationChange = (equation: Node[]): void => {
      this.setState({ equation });
    };

    handleVariableDelete = (): void => {
      this.props.onDelete(defined(this.props.viewElement.ident));
    };

    handleNotesChange = (_event: React.ChangeEvent<HTMLInputElement>): void => {};

    handleEquationCancel = (): void => {
      this.setState({
        equation: valueFromEquation(scalarEquationFor(this.props.variable)),
      });
    };

    handleEquationSave = (): void => {
      const { equation } = this.state;
      const initialEquation = scalarEquationFor(this.props.variable);

      const newEquation = equationFromValue(equation);
      if (initialEquation !== newEquation) {
        this.props.onEquationChange(defined(this.props.viewElement.ident), newEquation);
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
      const errorClass = this.props.classes.eqnError;
      return (
        <span {...props.attributes} className={isError ? errorClass : undefined}>
          {props.children}
        </span>
      );
    };

    renderEquation() {
      const { classes } = this.props;
      const { equation } = this.state;
      const initialEquation = scalarEquationFor(this.props.variable);

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
      const equationActionsEnabled = initialEquation !== equationFromValue(equation);

      const { left, right } = {
        left: 'dataMin',
        right: 'dataMax',
      };

      let chartOrErrors;
      const errors = this.props.variable.errors;
      if (errors) {
        const error = defined(errors.get(0));
        chartOrErrors = (
          <Typography className={classes.errorList}>error: {errorCodeDescription(error.code)}</Typography>
        );
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
            editor={this.state.editor}
            value={this.state.equation}
            onChange={this.handleEquationChange}
            onBlur={this.handleEquationSave}
          >
            <Editable
              className={classes.eqnEditor}
              renderLeaf={this.renderLeaf}
              placeholder="Enter an equation..."
              spellCheck={false}
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

          {/*<TextField*/}
          {/*  label="Units"*/}
          {/*  fullWidth*/}
          {/*  InputLabelProps={{*/}
          {/*    shrink: true,*/}
          {/*  }}*/}
          {/*  value={''}*/}
          {/*  margin="normal"*/}
          {/*/>*/}

          {/*<TextField*/}
          {/*  label="Notes"*/}
          {/*  fullWidth*/}
          {/*  InputLabelProps={{*/}
          {/*    shrink: true,*/}
          {/*  }}*/}
          {/*  value={defined(this.props.variable.xmile).doc}*/}
          {/*  onChange={this.handleNotesChange}*/}
          {/*  margin="normal"*/}
          {/*/>*/}

          {/*<br />*/}
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
