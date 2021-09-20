// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List, Map } from 'immutable';
import clsx from 'clsx';
import { Card, CardContent, Typography } from '@mui/material';
import { styled } from '@mui/material/styles';

import { SimError, ModelError, EquationError, ErrorCode, UnitError } from '@system-dynamics/core/datamodel';
import { errorCodeDescription } from '@system-dynamics/engine';

const SearchbarWidthSm = 359;
const SearchbarWidthMd = 420;
const SearchbarWidthLg = 480;

interface ErrorDetailsProps {
  simError: SimError | undefined;
  modelErrors: List<ModelError>;
  varErrors: Map<string, List<EquationError>>;
  varUnitErrors: Map<string, List<UnitError>>;
  status: 'ok' | 'error' | 'disabled';
}

export const ErrorDetails = styled(
  class InnerErrorDetails extends React.PureComponent<ErrorDetailsProps & { className?: string }> {
    render() {
      const { className, simError, modelErrors, varErrors, varUnitErrors } = this.props;
      const errors = [];
      if (
        simError &&
        !(
          (simError.code === ErrorCode.NotSimulatable || simError.code === ErrorCode.EmptyEquation) &&
          !modelErrors.isEmpty()
        )
      ) {
        errors.push(
          <Typography className="simlin-errordetails-list">
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
            <Typography className="simlin-errordetails-list">
              model error: {errorCodeDescription(err.code)}
              {details ? `: ${details}` : undefined}
            </Typography>,
          );
        }
      }
      for (const [ident, errs] of varErrors) {
        for (const err of errs) {
          errors.push(
            <Typography className="simlin-errordetails-list">
              variable "{ident}" error: {errorCodeDescription(err.code)}
            </Typography>,
          );
        }
      }
      for (const [ident, errs] of varUnitErrors) {
        for (const err of errs) {
          const details = err.details;
          errors.push(
            <Typography className="simlin-errordetails-list">
              variable "{ident}" unit error: {errorCodeDescription(err.code)}
              {details ? `: ${details}` : undefined}
            </Typography>,
          );
        }
      }

      return (
        <Card className={clsx(className, 'simlin-errordetails-card')} elevation={1}>
          <CardContent className="simlin-errordetails-inner">
            {errors.length > 0 ? (
              errors
            ) : (
              <Typography className="simlin-errordetails-yay">Your model is error free! 🎉</Typography>
            )}
          </CardContent>
        </Card>
      );
    }
  },
)(({ theme }) => ({
  '&.simlin-errordetails-card': {
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
  '.simlin-errordetails-inner': {
    paddingTop: 72,
  },
  '.simlin-errordetails-list': {
    color: '#cc0000',
  },
  '.simlin-errordetails-yay': {
    textAlign: 'center',
  },
}));
