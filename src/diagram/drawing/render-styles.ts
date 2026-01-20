// Static CSS content for SVG export rendering.
// These styles use hardcoded color values (not CSS variables) because standalone SVGs
// don't support CSS custom properties. The class names (simlin-*) are static and match
// those added to drawing components alongside their CSS module classes.

export const renderStyles = `
/* Canvas */
.simlin-canvas text {
  fill: #000000;
  font-size: 12px;
  font-family: "Roboto", "Open Sans", "Arial", sans-serif;
  font-weight: 300;
  text-anchor: middle;
  white-space: nowrap;
  vertical-align: middle;
}

/* Stock */
.simlin-stock rect {
  stroke-width: 1px;
  stroke: #000000;
  fill: #ffffff;
}

.simlin-stock.simlin-selected text {
  fill: #4444dd;
}

.simlin-stock.simlin-selected rect {
  stroke: #4444dd;
}

/* Auxiliary */
.simlin-aux circle {
  stroke-width: 1px;
  stroke: #000000;
  fill: #ffffff;
}

.simlin-aux.simlin-selected text {
  fill: #4444dd;
}

.simlin-aux.simlin-selected circle {
  stroke: #4444dd;
}

/* Flow */
.simlin-flow .simlin-outer {
  fill: none;
  stroke-width: 4px;
  stroke: #000000;
}

.simlin-flow .simlin-outer-selected {
  fill: none;
  stroke-width: 4px;
  stroke: #4444dd;
}

.simlin-flow .simlin-inner {
  fill: none;
  stroke-width: 2px;
  stroke: #ffffff;
}

.simlin-flow circle {
  stroke-width: 1px;
  fill: #ffffff;
  stroke: #000000;
}

.simlin-flow.simlin-selected text {
  fill: #4444dd;
}

.simlin-flow.simlin-selected circle {
  stroke: #4444dd;
}

/* Cloud */
path.simlin-cloud {
  stroke-width: 2px;
  stroke-linejoin: round;
  stroke-miterlimit: 4px;
  fill: #ffffff;
  stroke: #6388dc;
}

/* Alias */
.simlin-alias circle {
  stroke-width: 1px;
  stroke: #000000;
  fill: #ffffff;
}

.simlin-alias.simlin-selected text {
  fill: #4444dd;
}

.simlin-alias.simlin-selected circle {
  stroke: #4444dd;
}

/* Module */
.simlin-module rect {
  stroke-width: 1px;
  stroke: #000000;
  fill: #ffffff;
}

.simlin-module.simlin-selected text {
  fill: #4444dd;
}

.simlin-module.simlin-selected rect {
  stroke: #4444dd;
}

/* Connector */
.simlin-connector {
  stroke-width: 0.5px;
  stroke: gray;
  fill: none;
}

.simlin-connector-dashed {
  stroke-width: 0.5px;
  stroke: gray;
  stroke-dasharray: 2px;
  fill: none;
}

.simlin-connector-selected {
  stroke-width: 1px;
  stroke: #4444dd;
  fill: none;
}

.simlin-connector-bg {
  stroke-width: 7px;
  stroke: white;
  opacity: 0;
  fill: none;
}

/* Arrowhead */
path.simlin-arrowhead-flow {
  stroke-width: 1px;
  stroke-linejoin: round;
  stroke: #000000;
  fill: #ffffff;
}

path.simlin-arrowhead-flow.simlin-selected {
  stroke: #4444dd;
  fill: white;
}

path.simlin-arrowhead-link {
  stroke-width: 1px;
  stroke-linejoin: round;
  stroke: gray;
  fill: gray;
}

path.simlin-arrowhead-link.simlin-selected {
  stroke: #4444dd;
  fill: #4444dd;
}

path.simlin-arrowhead-bg {
  fill: white;
  opacity: 0;
}

/* Error indicators */
.simlin-error-indicator {
  stroke-width: 0px;
  fill: rgb(255, 152, 0);
}

/* Sparkline */
.simlin-sparkline-line {
  stroke-width: 0.5px;
  stroke-linecap: round;
  stroke: #2299dd;
  fill: none;
}

.simlin-sparkline-axis {
  stroke-width: 0.75px;
  stroke-linecap: round;
  stroke: #999;
  fill: none;
}
`;
