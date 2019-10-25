// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Node, Operation, Text, Value, ValueJSON } from 'slate';
import Plain from 'slate-plain-serializer';
import { Editor } from 'slate-react';

import { List } from 'immutable';

import Button from '@material-ui/core/Button';
import Card from '@material-ui/core/Card';
import CardActionArea from '@material-ui/core/CardActionArea';
import CardActions from '@material-ui/core/CardActions';
import CardContent from '@material-ui/core/CardContent';
import CardMedia from '@material-ui/core/CardMedia';
import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';
import TextField from '@material-ui/core/TextField';
import Typography from '@material-ui/core/Typography';

import { Variable } from '../../engine/vars';
import { ViewElement } from '../../engine/xmile';

import { defined, Series } from '../common';

import { HorizontalGridLines, LineSeries, VerticalGridLines, XAxis, XYPlot, YAxis } from 'react-vis';

const styles = createStyles({
  card: {
    width: 359,
  },
  cardInner: {
    paddingTop: 64,
  },
  plotFixup: {
    '& path': {
      fill: 'none',
    },
  },
  eqnEditor: {
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

export type VariableDetailsProps = Pick<VariableDetailsPropsFull, 'variable' | 'viewElement' | 'data'>;

interface VariableDetailsState {
  equation: Value;
}

function valueFromEquation(eqn: string): Value {
  return Plain.deserialize(eqn);
  /*
  const lines = eqn.split('\\n');
  const textNodes = lines.map(line => {
    return {
      object: 'block',
      type: 'code_line',
      nodes: [
        {
          object: 'text',
          text: line,
        },
      ],
    };
  });

  const doc: ValueJSON = {
    object: 'value',
    document: {
      object: 'document',
      nodes: [
        {
          object: 'block',
          type: 'code',
          data: {
            language: 'js',
          },
          nodes: textNodes,
        },
      ],
    },
  };

  return Value.fromJSON(doc);
   */
}

export const VariableDetails = withStyles(styles)(
  class InnerVariableDetails extends React.PureComponent<VariableDetailsPropsFull, VariableDetailsState> {
    constructor(props: VariableDetailsPropsFull) {
      super(props);

      const { variable } = props;

      this.state = {
        equation: valueFromEquation(defined(variable.xmile).eqn || ''),
      };
    }

    handleEquationChange = (change: { operations: List<Operation>; value: Value }): any => {
      this.setState({ equation: change.value });
      this.props.onEquationChange(this.props.viewElement.ident, Plain.serialize(this.state.equation));
    };

    handleNotesChange = (event: React.ChangeEvent<HTMLInputElement>) => {};

    render() {
      const { data, viewElement, variable, classes } = this.props;

      const series: Array<{ x: number; y: number }> = [];
      for (let i = 0; data && i < data.time.length; i++) {
        series.push({ x: data.time[i], y: data.values[i] });
      }

      const equationType = viewElement.type === 'stock' ? 'Initial Value' : 'Equation';

      return (
        <Card className={classes.card} elevation={1}>
          <CardContent className={classes.cardInner}>
            <Typography variant="body1" color="textSecondary" component="h3">
              {equationType}:
            </Typography>
            <Editor
              className={classes.eqnEditor}
              placeholder="Write an equation..."
              value={this.state.equation}
              onChange={this.handleEquationChange}
            />

            <TextField
              label="Units"
              fullWidth
              InputLabelProps={{
                shrink: true,
              }}
              value={''}
              margin="normal"
            />

            <TextField
              label="Notes"
              fullWidth
              InputLabelProps={{
                shrink: true,
              }}
              value={defined(this.props.variable.xmile).doc}
              onChange={this.handleNotesChange}
              margin="normal"
            />

            <br />
            <hr />
            <br />
            <div className={classes.plotFixup}>
              <XYPlot width={300} height={300}>
                <HorizontalGridLines />
                <VerticalGridLines />
                <XAxis />
                <YAxis marginLeft={50} />
                <LineSeries data={series} />
              </XYPlot>
            </div>
          </CardContent>
        </Card>
      );
    }
  },
);
