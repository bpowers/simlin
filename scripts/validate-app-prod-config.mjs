#!/usr/bin/env node

import fs from 'node:fs';
import process from 'node:process';
import { parseDocument } from 'yaml';

const BUILD_SCRIPT_KEY = 'GOOGLE_NODE_RUN_SCRIPTS';
const NODE_ENV_KEY = 'NODE_ENV';
const SESSION_KEY = 'authentication__seshcookie__key';

function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function formatYamlError(error) {
  const location = error.linePos?.[0];
  if (!location) {
    return error.message;
  }
  return `${error.message} at line ${location.line}, column ${location.col}`;
}

export function validateAppProdConfig(source, filename = '.app.prod.yaml') {
  const document = parseDocument(source, { prettyErrors: false });
  if (document.errors.length > 0) {
    return [
      {
        message: `failed to parse ${filename}: ${formatYamlError(document.errors[0])}`,
      },
    ];
  }

  const config = document.toJSON();
  const errors = [];

  if (!isRecord(config)) {
    errors.push({ message: `${filename} must contain a YAML mapping at the top level` });
    return errors;
  }

  const buildEnv = config.build_env_variables;
  if (!isRecord(buildEnv) || buildEnv[BUILD_SCRIPT_KEY] !== '') {
    errors.push({
      message: 'build_env_variables.GOOGLE_NODE_RUN_SCRIPTS must be set to an empty string',
    });
  }

  const env = config.env_variables;
  const nodeEnv = isRecord(env) ? env[NODE_ENV_KEY] : undefined;
  if (nodeEnv !== 'production') {
    errors.push({
      message: 'env_variables.NODE_ENV must be set to production',
    });
  }

  const sessionKey = isRecord(env) ? env[SESSION_KEY] : undefined;
  if (typeof sessionKey !== 'string' || sessionKey.trim() === '' || sessionKey.trim() === 'IN ENV') {
    errors.push({
      message: 'env_variables.authentication__seshcookie__key must be set to the existing production session key',
    });
  }

  // Cost cap: without max_instances a render storm or crash loop can fan out
  // F4 instances without bound (issue #694). The committed app.yaml carries
  // the reference value; the operator must mirror it here.
  const scaling = config.automatic_scaling;
  const maxInstances = isRecord(scaling) ? scaling.max_instances : undefined;
  if (!Number.isInteger(maxInstances) || maxInstances <= 0) {
    errors.push({
      message:
        'automatic_scaling.max_instances must be set to a positive integer (cost cap; mirror the committed app.yaml)',
    });
  }

  // Cross-origin embed contract (issue #688): third-party pages hotlink
  // sd-component.js, and its engine worker/WASM loads are cross-origin
  // requests against /static. Without the wildcard ACAO header the embed
  // silently fails to initialize the engine -- a regression no same-origin
  // smoke check can catch, so enforce the committed app.yaml's header here.
  const handlers = Array.isArray(config.handlers) ? config.handlers : [];
  const staticHandler = handlers.find((handler) => isRecord(handler) && handler.url === '/static');
  const headers = isRecord(staticHandler) ? staticHandler.http_headers : undefined;
  const allowOrigin = isRecord(headers) ? headers['Access-Control-Allow-Origin'] : undefined;
  if (allowOrigin !== '*') {
    errors.push({
      message:
        'handlers must include a /static handler with http_headers.Access-Control-Allow-Origin set to "*" (cross-origin embeds, issue #688; mirror the committed app.yaml)',
    });
  }

  return errors;
}

export function main(argv = process.argv) {
  const filename = argv[2];
  if (!filename) {
    console.error('usage: validate-app-prod-config.mjs <path-to-.app.prod.yaml>');
    return 2;
  }

  let source;
  try {
    source = fs.readFileSync(filename, 'utf8');
  } catch (error) {
    console.error(`ERROR: failed to read ${filename}: ${error.message}`);
    return 1;
  }

  const errors = validateAppProdConfig(source, filename);
  for (const error of errors) {
    console.error(`ERROR: ${error.message}`);
  }

  if (errors.length > 0) {
    console.error('       See the committed app.yaml reference and docs/dev/deploy.md.');
    return 1;
  }

  return 0;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  process.exitCode = main();
}
