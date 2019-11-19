// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { CartesianGrid, Line, LineChart, Tooltip, XAxis, YAxis } from 'recharts';

import { Operation, Value } from 'slate';
import Plain from 'slate-plain-serializer';
import { Editor } from 'slate-react';

import { List } from 'immutable';

import Button from '@material-ui/core/Button';
import Card from '@material-ui/core/Card';
import CardActions from '@material-ui/core/CardActions';
import CardContent from '@material-ui/core/CardContent';
import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';
import Tab from '@material-ui/core/Tab';
import Tabs from '@material-ui/core/Tabs';

import { Variable } from '../../engine/vars';
import { ViewElement } from '../../engine/xmile';

import { defined, Series } from '../common';

const styles = createStyles({
  card: {
    width: 359,
  },
  cardInner: {
    paddingTop: 52,
  },
  plotFixup: {},
  editorActions: {},
  eqnEditor: {
    backgroundColor: 'rgba(245, 245, 245)',
    borderRadius: 4,
    marginTop: 4,
    padding: 4,
    height: 80,
    fontFamily: "'Roboto Mono', monospace",
  },
});

interface VariableDetailsPropsFull extends WithStyles<typeof styles> {
  variable: Variable;
  viewElement: ViewElement;
  onEquationChange: (ident: string, newEquation: string) => void;
  data: Series | undefined;
}

// export type VariableDetailsProps = Pick<VariableDetailsPropsFull, 'variable' | 'viewElement' | 'data'>;

interface VariableDetailsState {
  equation: Value;
  activeTab: number;
}

function equationFromValue(value: Value): string {
  return Plain.serialize(value).trim();
}

function valueFromEquation(equation: string): Value {
  return Plain.deserialize(equation);
}

function equationFor(variable: Variable) {
  return (defined(variable.xmile).eqn || '').trim();
}

export const VariableDetails = withStyles(styles)(
  class InnerVariableDetails extends React.PureComponent<VariableDetailsPropsFull, VariableDetailsState> {
    constructor(props: VariableDetailsPropsFull) {
      super(props);

      const { variable } = props;

      this.state = {
        equation: valueFromEquation(equationFor(variable)),
        activeTab: 0,
      };
    }

    handleEquationChange = (change: { operations: List<Operation>; value: Value }): any => {
      this.setState({ equation: change.value });
    };

    handleNotesChange = (event: React.ChangeEvent<HTMLInputElement>) => {};

    handleEquationCancel = () => {
      this.setState({
        equation: valueFromEquation(equationFor(this.props.variable)),
      });
    };

    handleEquationSave = () => {
      const { equation } = this.state;
      const initialEquation = equationFor(this.props.variable);

      const newEquation = equationFromValue(equation);
      if (initialEquation !== newEquation) {
        this.props.onEquationChange(this.props.viewElement.ident, newEquation);
      }
    };

    formatValue = (value: string | number | Array<string | number>) => {
      return typeof value === 'number' ? value.toFixed(3) : value;
    };

    handleTabChange = (event: React.ChangeEvent<{}>, newValue: number) => {
      this.setState({ activeTab: newValue });
    };

    renderEquation() {
      const { data, classes } = this.props;
      const { equation } = this.state;
      const initialEquation = equationFor(this.props.variable);

      let yMin = 0;
      let yMax = 0;
      const series: Array<{ x: number; y: number }> = [];
      for (let i = 0; data && i < data.time.length; i++) {
        const x = data.time[i];
        const y = data.values[i];
        series.push({ x, y });
        if (y < yMin) {
          yMin = y;
        }
        if (y > yMax) {
          yMax = y;
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

      return (
        <div>
          <Editor
            autoFocus
            className={classes.eqnEditor}
            placeholder="Enter an equation..."
            value={this.state.equation}
            onChange={this.handleEquationChange}
            onBlur={this.handleEquationSave}
          />

          <CardActions className={classes.editorActions}>
            <Button size="small" color="primary" disabled={!equationActionsEnabled} onClick={this.handleEquationCancel}>
              Cancel
            </Button>
            <Button size="small" color="primary" disabled={!equationActionsEnabled} onClick={this.handleEquationSave}>
              Save
            </Button>
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
          <div className={classes.plotFixup}>
            <LineChart
              width={327}
              height={300}
              data={series}
              // onMouseDown = { (e) => this.setState({refAreaLeft:e.activeLabel}) }
              // onMouseMove = { (e) => this.state.refAreaLeft && this.setState({refAreaRight:e.activeLabel}) }
              // onMouseUp = { this.zoom.bind( this ) }
            >
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
              <Line yAxisId="1" type="linear" dataKey="y" stroke="#8884d8" animationDuration={300} />
            </LineChart>
          </div>
        </div>
      );
    }

    renderLookup() {
      return undefined;
    }

    render() {
      const { classes, viewElement } = this.props;
      const { activeTab } = this.state;

      const equationType = viewElement.type === 'stock' ? 'Initial Value' : 'Equation';
      const content = activeTab === 0 ? this.renderEquation() : this.renderLookup();
      const lookupTab = viewElement.type === 'stock' ? undefined : <Tab label="Lookup Function" />;

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

          <CardContent>{content}</CardContent>
        </Card>
      );
    }
  },
);
