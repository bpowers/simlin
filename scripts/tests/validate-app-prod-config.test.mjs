import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { validateAppProdConfig } from '../validate-app-prod-config.mjs';

const MAX_INSTANCES_MESSAGE =
  'automatic_scaling.max_instances must be set to a positive integer (cost cap; mirror the committed app.yaml)';

// Every fixture that isn't specifically exercising the max_instances check
// carries this block so its expected messages stay focused on one concern.
const scalingBlock = `
automatic_scaling:
  max_instances: 8
`;

const validConfig = `
runtime: nodejs24

build_env_variables:
  GOOGLE_NODE_RUN_SCRIPTS: ''

env_variables:
  NODE_ENV: production
  authentication__seshcookie__key: production-secret
${scalingBlock}`;

function messagesFor(source) {
  return validateAppProdConfig(source, '.app.prod.yaml').map((error) => error.message);
}

describe('validateAppProdConfig', () => {
  it('accepts a production config with deploy-critical values in the correct sections', () => {
    assert.deepEqual(messagesFor(validConfig), []);
  });

  it('rejects deploy guard tokens that appear only in comments', () => {
    const messages = messagesFor(`
# build_env_variables.GOOGLE_NODE_RUN_SCRIPTS: ''
# env_variables.authentication__seshcookie__key: production-secret
# automatic_scaling.max_instances: 8
runtime: nodejs24
`);

    assert.deepEqual(messages, [
      'build_env_variables.GOOGLE_NODE_RUN_SCRIPTS must be set to an empty string',
      'env_variables.NODE_ENV must be set to production',
      'env_variables.authentication__seshcookie__key must be set to the existing production session key',
      MAX_INSTANCES_MESSAGE,
    ]);
  });

  it('rejects GOOGLE_NODE_RUN_SCRIPTS outside build_env_variables', () => {
    const messages = messagesFor(`
env_variables:
  NODE_ENV: production
  GOOGLE_NODE_RUN_SCRIPTS: ''
  authentication__seshcookie__key: production-secret
${scalingBlock}`);

    assert.deepEqual(messages, ['build_env_variables.GOOGLE_NODE_RUN_SCRIPTS must be set to an empty string']);
  });

  it('rejects a non-empty GOOGLE_NODE_RUN_SCRIPTS value', () => {
    const messages = messagesFor(`
build_env_variables:
  GOOGLE_NODE_RUN_SCRIPTS: build
env_variables:
  NODE_ENV: production
  authentication__seshcookie__key: production-secret
${scalingBlock}`);

    assert.deepEqual(messages, ['build_env_variables.GOOGLE_NODE_RUN_SCRIPTS must be set to an empty string']);
  });

  it('rejects session keys outside env_variables', () => {
    const messages = messagesFor(`
build_env_variables:
  GOOGLE_NODE_RUN_SCRIPTS: ''
  authentication__seshcookie__key: production-secret
env_variables:
  NODE_ENV: production
${scalingBlock}`);

    assert.deepEqual(messages, [
      'env_variables.authentication__seshcookie__key must be set to the existing production session key',
    ]);
  });

  it('rejects blank or placeholder session keys', () => {
    for (const value of ["''", 'IN ENV', "' IN ENV '"]) {
      assert.deepEqual(
        messagesFor(`
build_env_variables:
  GOOGLE_NODE_RUN_SCRIPTS: ''
env_variables:
  NODE_ENV: production
  authentication__seshcookie__key: ${value}
${scalingBlock}`),
        ['env_variables.authentication__seshcookie__key must be set to the existing production session key'],
      );
    }
  });

  it('rejects missing NODE_ENV', () => {
    const messages = messagesFor(`
build_env_variables:
  GOOGLE_NODE_RUN_SCRIPTS: ''
env_variables:
  authentication__seshcookie__key: production-secret
${scalingBlock}`);

    assert.deepEqual(messages, ['env_variables.NODE_ENV must be set to production']);
  });

  it('rejects NODE_ENV outside env_variables or set to a non-production value', () => {
    for (const source of [
      `
build_env_variables:
  GOOGLE_NODE_RUN_SCRIPTS: ''
  NODE_ENV: production
env_variables:
  authentication__seshcookie__key: production-secret
${scalingBlock}`,
      `
build_env_variables:
  GOOGLE_NODE_RUN_SCRIPTS: ''
env_variables:
  NODE_ENV: development
  authentication__seshcookie__key: production-secret
${scalingBlock}`,
    ]) {
      assert.deepEqual(messagesFor(source), ['env_variables.NODE_ENV must be set to production']);
    }
  });

  it('rejects a missing automatic_scaling block', () => {
    const messages = messagesFor(`
build_env_variables:
  GOOGLE_NODE_RUN_SCRIPTS: ''
env_variables:
  NODE_ENV: production
  authentication__seshcookie__key: production-secret
`);

    assert.deepEqual(messages, [MAX_INSTANCES_MESSAGE]);
  });

  it('rejects max_instances values that are not positive integers', () => {
    for (const value of ['0', '-2', '2.5', 'unlimited', "''"]) {
      assert.deepEqual(
        messagesFor(`
build_env_variables:
  GOOGLE_NODE_RUN_SCRIPTS: ''
env_variables:
  NODE_ENV: production
  authentication__seshcookie__key: production-secret
automatic_scaling:
  max_instances: ${value}
`),
        [MAX_INSTANCES_MESSAGE],
        `max_instances: ${value} should be rejected`,
      );
    }
  });

  it('rejects malformed YAML', () => {
    const messages = messagesFor(`
build_env_variables:
  GOOGLE_NODE_RUN_SCRIPTS: ''
env_variables:
  authentication__seshcookie__key: [unterminated
`);

    assert.equal(messages.length, 1);
    assert.match(messages[0], /^failed to parse \.app\.prod\.yaml:/);
  });
});
