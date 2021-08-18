module.exports = {
  presets: [
    require.resolve('@docusaurus/core/lib/babel/preset'),
    "@babel/typescript",
    {
      "exclude": [
        "transform-runtime",
        "transform-regenerator"
      ]
    }
  ],
  plugins: [
    [
      "@babel/plugin-proposal-class-properties",
      {
        "loose": true
      }
    ],
    [
      "@babel/plugin-proposal-optional-chaining",
      {}
    ],
    [
      "@babel/plugin-proposal-nullish-coalescing-operator",
      {}
    ],
    [
      "@babel/plugin-proposal-private-property-in-object",
      {
        "loose": true
      }
    ]
  ],
};
