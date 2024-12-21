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
      "@babel/plugin-transform-class-properties",
      {
        "loose": true
      }
    ],
    [
      "@babel/plugin-transform-optional-chaining",
      {}
    ],
    [
      "@babel/plugin-transform-nullish-coalescing-operator",
      {}
    ],
    [
      "@babel/plugin-transform-private-property-in-object",
      {
        "loose": true
      }
    ]
  ],
};
