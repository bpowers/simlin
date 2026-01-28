// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List, Map } from 'immutable';

import { SimError, ModelError, EquationError, ErrorCode, UnitError } from '@system-dynamics/core/datamodel';
import { errorCodeDescription } from '@system-dynamics/engine2';

import styles from './ErrorDetails.module.css';

interface ErrorDetailsProps {
  simError: SimError | undefined;
  modelErrors: List<ModelError>;
  varErrors: Map<string, List<EquationError>>;
  varUnitErrors: Map<string, List<UnitError>>;
  status: 'ok' | 'error' | 'disabled';
}

export class ErrorDetails extends React.PureComponent<ErrorDetailsProps> {
  render() {
    const { simError, modelErrors, varErrors, varUnitErrors } = this.props;
    const errors = [];
    if (
      simError &&
      !(
        (simError.code === ErrorCode.NotSimulatable || simError.code === ErrorCode.EmptyEquation) &&
        !modelErrors.isEmpty()
      )
    ) {
      errors.push(
        <div className={styles.list}>simulation error: {errorCodeDescription(simError.code)}</div>,
      );
    }
    if (!modelErrors.isEmpty()) {
      for (const err of modelErrors) {
        if (err.code === ErrorCode.VariablesHaveErrors && !varErrors.isEmpty()) {
          continue;
        }
        const details = err.details;
        errors.push(
          <div className={styles.list}>
            model error: {errorCodeDescription(err.code)}
            {details ? `: ${details}` : undefined}
          </div>,
        );
      }
    }
    for (const [ident, errs] of varErrors) {
      for (const err of errs) {
        errors.push(
          <div className={styles.list}>
            variable "{ident}" error: {errorCodeDescription(err.code)}
          </div>,
        );
      }
    }
    for (const [ident, errs] of varUnitErrors) {
      for (const err of errs) {
        const details = err.details;
        errors.push(
          <div className={styles.list}>
            variable "{ident}" unit error: {errorCodeDescription(err.code)}
            {details ? `: ${details}` : undefined}
          </div>,
        );
      }
    }

    return (
      <div className={styles.card}>
        <div className={styles.inner}>
          {errors.length > 0 ? errors : <div className={styles.yay}>Your model is error free!</div>}
        </div>
      </div>
    );
  }
}
