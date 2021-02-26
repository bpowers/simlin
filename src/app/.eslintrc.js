module.exports = {
  "env": {
    "browser": true,
    "es6": true,
    "node": true,
  },
  "extends": [
    "plugin:@typescript-eslint/recommended",
    "plugin:@typescript-eslint/recommended-requiring-type-checking",
    "plugin:react/recommended",
    "prettier"
  ],
  "parser": "@typescript-eslint/parser",
  "parserOptions": {
    "project": "tsconfig.browser.json"
  },
  "rules": {
    "@typescript-eslint/explicit-function-return-type": "off",
    "@typescript-eslint/no-empty-function": "off",
    "@typescript-eslint/no-use-before-define": "off",
    "@typescript-eslint/no-explicit-any": "off",
    "@typescript-eslint/restrict-template-expressions": "off",
    "@typescript-eslint/no-unsafe-member-access": "off",
    "@typescript-eslint/no-unused-vars": [
      "warn",
      {
        "argsIgnorePattern": "^_",
      },
    ],
  },
  "settings": {
    "react": {
      "version": "detect"
    }
  },
  "ignorePatterns": [
    "src/importer/",
    "src/engine-v2/",
    "src/system-dynamics-engine/",
    "src/schemas/",
  ],
};
