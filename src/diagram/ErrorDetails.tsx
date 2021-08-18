// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List, Map } from 'immutable';

import { Card, CardContent, Typography } from '@material-ui/core';
import { createStyles, withStyles, WithStyles, Theme } from '@material-ui/core/styles';

import { SimError, ModelError, EquationError, ErrorCode } from '@system-dynamics/core/datamodel';

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
      paddingTop: 72,
    },
    errorList: {
      color: '#cc0000',
    },
    yay: {
      textAlign: 'center',
    },
  });

interface ErrorDetailsPropsFull extends WithStyles<typeof styles> {
  simError: SimError | undefined;
  modelErrors: List<ModelError>;
  varErrors: Map<string, List<EquationError>>;
  varUnitErrors: Map<string, List<EquationError>>;
  status: 'ok' | 'error' | 'disabled';
}

// export type ErrorDetailsProps = Pick<ErrorDetailsPropsFull, 'variable' | 'viewElement' | 'data'>;

export const ErrorDetails = withStyles(styles)(
  class InnerErrorDetails extends React.PureComponent<ErrorDetailsPropsFull> {
    render() {
      const { classes, simError, modelErrors, varErrors, varUnitErrors } = this.props;
      const errors = [];
      if (
        simError &&
        !(
          (simError.code === ErrorCode.NotSimulatable || simError.code === ErrorCode.EmptyEquation) &&
          !modelErrors.isEmpty()
        )
      ) {
        errors.push(
          <Typography className={classes.errorList}>
            simulation error: {errorCodeDescription(simError.code)}
          </Typography>,
        );
      }
      if (!modelErrors.isEmpty()) {
        for (const err of modelErrors) {
          // don't yell multiple times about the same thing
          if (err.code === ErrorCode.VariablesHaveErrors && !varErrors.isEmpty()) {
            continue;
          }
          const details = err.details;
          errors.push(
            <Typography className={classes.errorList}>
              model error: {errorCodeDescription(err.code)}
              {details ? `: ${details}` : undefined}
            </Typography>,
          );
        }
      }
      for (const [ident, errs] of varErrors) {
        for (const err of errs) {
          errors.push(
            <Typography className={classes.errorList}>
              variable "{ident}" error: {errorCodeDescription(err.code)}
            </Typography>,
          );
        }
      }
      for (const [ident, errs] of varUnitErrors) {
        for (const err of errs) {
          errors.push(
            <Typography className={classes.errorList}>
              variable "{ident}" unit error: {errorCodeDescription(err.code)}
            </Typography>,
          );
        }
      }

      return (
        <Card className={classes.card} elevation={1}>
          <CardContent className={classes.cardInner}>
            {errors.length > 0 ? errors : <Typography className={classes.yay}>Your model is error free! 🎉</Typography>}
          </CardContent>
        </Card>
      );
    }
  },
);
