const eslint = require('@eslint/js');
const tseslint = require('@typescript-eslint/eslint-plugin');
const tsParser = require('@typescript-eslint/parser');
const reactPlugin = require('eslint-plugin-react');
const prettierConfig = require('eslint-config-prettier');

const baseConfig = {
  files: ['**/*.ts', '**/*.tsx'],
  languageOptions: {
    parser: tsParser,
    parserOptions: {
      ecmaVersion: 2020,
      sourceType: 'module',
    },
    globals: {
      console: 'readonly',
      process: 'readonly',
      Buffer: 'readonly',
      __dirname: 'readonly',
      __filename: 'readonly',
      exports: 'writable',
      module: 'writable',
      require: 'readonly',
      global: 'readonly',
      URL: 'readonly',
    },
  },
  plugins: {
    '@typescript-eslint': tseslint,
  },
  rules: {
    ...eslint.configs.recommended.rules,
    ...tseslint.configs.recommended.rules,
    '@typescript-eslint/explicit-function-return-type': 'off',
    '@typescript-eslint/no-empty-function': 'off',
    '@typescript-eslint/no-use-before-define': 'off',
    '@typescript-eslint/no-explicit-any': 'off',
    '@typescript-eslint/restrict-template-expressions': 'off',
    '@typescript-eslint/no-unsafe-member-access': 'off',
    '@typescript-eslint/no-unused-vars': [
      'warn',
      {
        argsIgnorePattern: '^_',
      },
    ],
    // Disable rules that conflict with prettier
    ...prettierConfig.rules,
  },
};

const reactConfig = {
  files: ['**/*.tsx'],
  plugins: {
    react: reactPlugin,
  },
  rules: {
    ...reactPlugin.configs.recommended.rules,
  },
  settings: {
    react: {
      version: 'detect',
    },
  },
};

const createConfig = (options = {}) => {
  const configs = [];

  // Add ignore patterns first if provided
  if (options.ignorePatterns) {
    configs.push({
      ignores: options.ignorePatterns,
    });
  }

  configs.push(baseConfig);

  if (options.react) {
    configs.push(reactConfig);
  }

  if (options.project) {
    configs[configs.length - (options.react ? 2 : 1)].languageOptions.parserOptions.project = options.project;
  }

  return configs;
};

module.exports = { createConfig };