# Diagram Layout Documentation

This document provides comprehensive information about how system dynamics diagrams are laid out and rendered in Simlin. It describes the coordinate system, element types, their visual properties, and layout rules to enable correct generation of Views with ViewElements.

## Table of Contents
- [Coordinate System](#coordinate-system)
- [View Structure](#view-structure)
- [Element Types](#element-types)
  - [Stock Elements](#stock-elements)
  - [Flow Elements](#flow-elements)
  - [Auxiliary Variables](#auxiliary-variables)
  - [Module Elements](#module-elements)
  - [Cloud Elements](#cloud-elements)
  - [Link/Connector Elements](#linkconnector-elements)
  - [Alias Elements](#alias-elements)
- [Label Positioning](#label-positioning)
- [Element Connections and Constraints](#element-connections-and-constraints)
- [Special Rendering Cases](#special-rendering-cases)
- [Constants and Dimensions](#constants-and-dimensions)
- [Rendering Architecture](#rendering-architecture)
- [Hit Testing and Selection](#hit-testing-and-selection)

## Coordinate System

### Basic Properties
- **Origin**: Top-left corner at (0,0)
- **X-axis**: Increases to the right
- **Y-axis**: Increases downward (standard SVG/web coordinate system)
- **Units**: All coordinates and dimensions are in pixels
- **Zoom**: Views have a zoom property that scales the entire canvas

### Coordinate Transformation
- Screen to canvas conversion uses matrix transformation with zoom factor
- Implementation in `screenToCanvasPoint` function:
  ```typescript
  canvasPoint = screenPoint.matrixTransform(
    new DOMMatrix([zoom, 0, 0, zoom, 0, 0]).inverse()
  )
  ```
- Elements store their position as center coordinates (cx, cy)
- For positioned elements (Stock, Aux, Module, Flow valve), x/y are aliased to cx/cy

## View Structure

### StockFlowView
The main container for a system dynamics diagram with these properties:
```typescript
{
  nextUid: number,              // Next available UID for new elements
  elements: List<ViewElement>,  // All visual elements in the view
  viewBox: Rect,                // Bounding box of the view
  zoom: number                   // Zoom factor (negative means auto-fit)
}
```

### ViewBox/Rect
Defines rectangular boundaries:
```typescript
{
  top: number,
  left: number,
  right: number,
  bottom: number
}
```

## Element Types

All elements implement the `ViewElement` interface with these common properties:
- `uid`: Unique identifier (number)
- `cx/cy`: Center coordinates (computed from x/y for positioned elements)
- `ident`: Canonical identifier string (optional, defined for named elements)
- `isZeroRadius`: Whether element has zero visual radius (affects connector attachment)
- `isNamed()`: Method indicating if element has a name/ident

### Stock Elements

**Visual Representation**: Rectangle box representing accumulation/state
- **Dimensions**: 45×35 pixels (StockWidth × StockHeight)
- **Shape**: Rectangle with 1px black stroke, white fill
- **Position**: Stored as (x,y) center coordinates

**Data Structure**:
```typescript
StockViewElement {
  uid: number,
  name: string,             // Display name
  ident: string,            // Canonical identifier
  var?: Stock,              // Associated variable
  x: number,                // Center X coordinate
  y: number,                // Center Y coordinate
  labelSide: LabelSide,     // 'top'|'bottom'|'left'|'right'|'center'
  isZeroRadius: boolean,    // Always false for stocks
  inflows: List<UID>,       // UIDs of inflowing flows (computed)
  outflows: List<UID>       // UIDs of outflowing flows (computed)
}
```

**Special Rendering**:
- **Arrayed stocks**: Show 3 stacked rectangles offset by 3px
  - Back rectangle: offset (+3, +3)
  - Middle rectangle: original position
  - Front rectangle: offset (-3, -3)
- **Warning indicator**: Orange circle (rgb(255, 152, 0)), radius 3px at top-right corner
  - Position: (cx + width/2 - 1, cy - height/2 + 1)
- **Sparkline**: Shows time series data within the rectangle
  - Positioned 1px inset from edges
  - Size: (width - 2) × (height - 2)
- **Selection**: Blue stroke (#4444dd) when selected
- **Effective radius for connectors**: 15px (used for intersection calculations)

### Flow Elements

**Visual Representation**: Pipe with valve (circle) and directional arrow
- **Valve**: Circle with radius 9px (AuxRadius) at flow center (x,y)
- **Pipe**: Double-line rendering:
  - Outer: 4px thick black stroke
  - Inner: 2px thick white stroke (creates pipe appearance)
- **Arrowhead**: 8px radius (FlowArrowheadRadius) at destination end

**Data Structure**:
```typescript
FlowViewElement {
  uid: number,
  name: string,
  ident: string,
  var?: Flow,
  x: number,                // Valve center X
  y: number,                // Valve center Y
  labelSide: LabelSide,
  points: List<Point>,      // Path points defining the flow
  isZeroRadius: boolean     // Always false for flows
}

Point {
  x: number,
  y: number,
  attachedToUid?: number    // UID of connected element (stock/cloud)
}
```

**Flow Path Rules**:
1. **Must have at least 2 points** (source and destination)
2. **First point** connects to source (stock or cloud)
3. **Last point** connects to sink (stock or cloud)
4. **Intermediate points** define the path shape (currently only 2-point flows supported)
5. **Flows are constrained** to horizontal or vertical segments when connected to stocks
6. **Stock connections**:
   - Endpoints snap to stock edges (±width/2 or ±height/2)
   - Must be at least 3px from corners
7. **Cloud connections**:
   - Attach at CloudRadius (13.5px) distance from cloud center
   - Clouds adjust position when dragged

**Path Rendering**:
- Path is drawn as SVG path using M (move) and L (line) commands
- Final segment adjusted by 7.5px (finalAdjust) to accommodate arrowhead
- Arrowhead angle snapped to cardinal directions (0°, 90°, 180°, 270°)

**Movement Constraints**:
- **Horizontal flows**: All points have same y-coordinate
- **Vertical flows**: All points have same x-coordinate
- **Valve movement**: Constrained within bounds between connected elements
- **Cloud-to-cloud flows**: Can be diagonal (no axis constraint)

### Auxiliary Variables

**Visual Representation**: Circle for scalar values and parameters
- **Dimensions**: Circle with radius 9px (AuxRadius)
- **Shape**: Circle with 1px black stroke, white fill
- **Position**: Stored as (x,y) center coordinates

**Data Structure**:
```typescript
AuxViewElement {
  uid: number,
  name: string,
  ident: string,
  var?: Aux,
  x: number,                // Center X
  y: number,                // Center Y
  labelSide: LabelSide,
  isZeroRadius: boolean     // Can be true for invisible junction points
}
```

**Special Rendering**:
- **Arrayed variables**: Show 3 stacked circles offset by 3px
  - Back circle: offset (+3, +3)
  - Middle circle: original position
  - Front circle: offset (-3, -3)
- **Warning indicator**: Orange circle at 45° angle (-π/4 radians) from center
  - Position calculated: (cx + r*cos(θ), cy + r*sin(θ))
- **Sparkline**: Miniature time series chart within circle
  - Positioned to fit within circle bounds
- **Hit testing**: Point is inside if distance from center ≤ AuxRadius

### Module Elements

**Visual Representation**: Rounded rectangle representing sub-model
- **Dimensions**: 55×45 pixels (ModuleWidth × ModuleHeight)
- **Shape**: Rectangle with 5px corner radius (ModuleRadius)
- **Stroke**: 1px black, white fill

**Data Structure**:
```typescript
ModuleViewElement {
  uid: number,
  name: string,
  ident: string,
  var?: Module,
  x: number,                // Center X
  y: number,                // Center Y
  labelSide: LabelSide,
  isZeroRadius: boolean     // Always false for modules
}
```

**Special Properties**:
- **Effective radius for connectors**: 25px (used for arc intersection calculations)
- **No array rendering**: Modules don't show stacked shapes for arrays
- **No sparklines**: Modules don't display embedded charts

### Cloud Elements

**Visual Representation**: Cloud shape indicating infinite source/sink
- **Dimensions**: Effective radius of 13.5px (CloudRadius)
- **Base SVG path**: 55px width (CloudWidth), scaled to fit radius
- **Shape**: Predefined SVG path (CloudPath) resembling a cloud
- **Stroke**: 2px width, color varies by theme:
  - Light mode: #6388dc
  - Dark mode: #2D498A
- **Fill**: White

**Data Structure**:
```typescript
CloudViewElement {
  uid: number,
  flowUid: number,          // UID of associated flow (required)
  x: number,                // Center X
  y: number,                // Center Y (note: not cx/cy)
  isZeroRadius: boolean     // Always false for clouds
}
```

**Special Properties**:
- **No label**: Clouds are unnamed (no name or ident property)
- **Always attached**: Must be connected to exactly one flow
- **Position constraints**:
  - When flow is horizontal: cloud can only move along x-axis
  - When flow is vertical: cloud can only move along y-axis
  - For diagonal flows (cloud-to-cloud): no movement constraints
- **Scaling**: SVG path scaled using matrix transform to achieve target diameter
  - Scale factor = (2 × CloudRadius) / CloudWidth

### Link/Connector Elements

**Visual Representation**: Curved or straight arrow connecting elements
- **Shape**: Path with arrowhead at destination
- **Stroke**:
  - Normal: 0.5px gray
  - Selected: 1px blue (#4444dd)
  - Dashed (for special relationships): 2px dash array
- **Arrowhead**: 6px radius (ArrowheadRadius)
- **Background path**: Invisible wider path for easier selection

**Data Structure**:
```typescript
LinkViewElement {
  uid: number,
  fromUid: number,          // Source element UID
  toUid: number,            // Destination element UID
  arc?: number,             // Takeoff angle in radians (undefined = straight)
  isStraight: boolean,      // Force straight line
  multiPoint?: List<Point>  // Multi-segment connectors (future/not implemented)
}
```

**Path Types and Calculation**:

1. **Straight lines**:
   - When `arc` is undefined OR
   - When angle difference < 6° (StraightLineMax in degrees)
   - Direct line from source edge to destination edge

2. **Curved paths (Arcs)**:
   - Circular arc with specified takeoff angle
   - Algorithm:
     1. Calculate perpendicular to takeoff angle at source
     2. Find perpendicular bisector of source-destination line
     3. Intersection gives circle center
     4. Calculate radius from center to source
     5. Determine sweep direction based on angle span
   - SVG arc path parameters: radius, sweep-flag, large-arc-flag

**Intersection Calculation with Elements**:
- Elements have different effective radii for connector attachment:
  - **Stocks**: 15px effective radius
  - **Modules**: 25px effective radius
  - **Aux/Flow valves**: 9px (AuxRadius)
  - **Zero-radius elements**: 0px
- Intersection point calculated using tangent offset from element center

**Special Properties**:
- **No center coordinates**: cx/cy return NaN (connectors don't have position)
- **No ident**: Links are unnamed (ident returns undefined)
- **isDashed property**: Can render with dashed stroke for special relationships

### Alias Elements

**Visual Representation**: Dashed circle referencing another variable
- **Dimensions**: Circle with radius 9px (same as Aux)
- **Shape**: Circle with dashed stroke (stroke-dasharray: 2px)
- **Label**: Shows name of referenced element (not its own name)

**Data Structure**:
```typescript
AliasViewElement {
  uid: number,
  aliasOfUid: number,       // UID of referenced element
  x: number,                // Center X
  y: number,                // Center Y
  labelSide: LabelSide,
  isZeroRadius: boolean     // Can be true for invisible references
}
```

**Special Properties**:
- **No name/ident**: Aliases don't have their own name
- **Label from reference**: Display name comes from aliasOf element
- **No array rendering**: Aliases don't show stacked shapes
- **Sparklines supported**: Can display time series of referenced variable
- **Hit testing**: Same as Aux (distance ≤ AuxRadius)

## Label Positioning

Labels can be positioned relative to their element using the `LabelSide` property:

### Label Sides
- **'top'**: Above element, centered horizontally
- **'bottom'**: Below element, centered horizontally
- **'left'**: Left of element, right-aligned text
- **'right'**: Right of element, left-aligned text
- **'center'**: Centered on element (default for stocks/flows, overlays the element)

### Label Layout Calculation

**Constants**:
```typescript
const LabelPadding = 3;   // Space between element and label text
const lineSpacing = 14;   // Vertical space between lines of text
```

**Positioning Algorithm**:
```typescript
switch(side) {
  case 'top':
    x = elementCenterX;
    y = elementCenterY - elementRadius - LabelPadding - textHeight;
    textAnchor = 'middle';
    reverseBaseline = true;  // Lines stack upward
    break;
  case 'bottom':
    x = elementCenterX;
    y = elementCenterY + elementRadius + LabelPadding;
    textAnchor = 'middle';
    reverseBaseline = false;
    break;
  case 'left':
    x = elementCenterX - elementRadius - LabelPadding;
    y = elementCenterY - (12 + (lineCount - 1) * 14) / 2 - 3;
    textAnchor = 'end';
    break;
  case 'right':
    x = elementCenterX + elementRadius + LabelPadding;
    y = elementCenterY - (12 + (lineCount - 1) * 14) / 2 - 3;
    textAnchor = 'start';
    break;
}
```

### Multi-line Labels
- Lines separated by '\n' character in the name string
- Line spacing: 14px between baselines
- **Top-positioned labels**: Use reverse baseline - first line is positioned highest
- **SVG tspan elements**: Each line rendered as separate tspan with dy offset

### Label Bounds Calculation
- **Width estimation**: `maxLineCharacters × 6px + 10px` padding
- **Height**: `lineCount × 14px`
- **Bounds include** element bounds plus label bounds for complete bounding box

### Display Name Processing
- **Underscores to spaces**: `initial_inventory` → `initial inventory`
- **Newline support**: `\n` in names creates multi-line labels
- Implemented by `displayName()` function

## Element Connections and Constraints

### Flow Connections

Flows connect stocks and clouds with specific constraints:

1. **Stock-to-Stock flows**:
   - **Axis constraint**: Must maintain horizontal OR vertical orientation
   - **Endpoint attachment**: Snap to stock edge (±StockWidth/2 or ±StockHeight/2)
   - **Valve position**: Constrained within connection bounds
   - **Corner clearance**: Minimum 3px from stock corners
   - **Movement**: When stock moves, flow endpoints adjust to maintain connection

2. **Cloud-to-Stock flows**:
   - **Cloud position**: Adjustable along flow axis only
   - **Distance maintenance**: Cloud maintains CloudRadius distance from flow endpoint
   - **Axis determination**: Based on relative positions at creation time

3. **Cloud-to-Cloud flows**:
   - **No axis constraint**: Can be diagonal
   - **Free movement**: Both clouds and valve can move freely
   - **Uniform translation**: All points move together when valve dragged

### Flow Movement Algorithm

**UpdateStockAndFlows**: When moving a stock with connected flows:
1. Classify flows by attachment side (left, right, top, bottom)
2. Calculate proposed new stock position
3. Constrain position to keep flows valid:
   - Horizontal flows: constrain Y within flow valve ± StockHeight/2 - 3px
   - Vertical flows: constrain X within flow valve ± StockWidth/2 - 3px
4. Adjust all flow endpoints to new stock edges

**UpdateFlow**: When moving a flow valve:
1. Determine if flow is horizontal, vertical, or diagonal
2. For stock-connected flows:
   - Maintain axis alignment
   - Constrain valve position within valid range
3. Update cloud positions to follow flow movement
4. Keep minimum 20px clearance from flow endpoints

### Connector Rules

Links/connectors between elements follow these rules:

1. **Attachment points**:
   - Calculate intersection with element's effective boundary
   - Use element-specific effective radius
   - Account for element shape (circle vs rectangle)

2. **Path determination**:
   - **Straight**: When no arc specified or angle < 6°
   - **Curved**: Circular arc with takeoff angle
   - **Multi-point**: Defined but not implemented

3. **Visual feedback**:
   - Invisible wider background path for easier selection
   - Arrowhead indicates direction of influence
   - Optional dashed style for special relationships

## Special Rendering Cases

### Arrayed Elements

Elements representing array variables show multiple stacked shapes:
- **3 shapes total**: back, middle (main), front
- **3px offset** between layers (diagonal offset)
- **Front shape** is the interactive element
- **Applies to**: Stocks, Aux variables, Flows
- **Not for**: Modules, Aliases, Clouds
- **Label adjustment**: Account for array offset in label positioning

### Zero Radius Elements

Special elements with `isZeroRadius = true`:
- **No visual representation** at stored position
- **Connectors attach** directly to center point (0px effective radius)
- **Used for** invisible junction points or hidden elements
- **Can be**: Auxiliary or Alias elements

### Selection States

Selected elements show visual feedback:
- **Stroke color**: Blue (#4444dd)
- **Text labels**: Blue color when parent selected
- **Connectors**: Thicker stroke (1px vs 0.5px)
- **Maintained during drag**: Selection persists through interactions

### Target Validation

During drag operations, valid/invalid drop targets show:
- **Valid target**:
  - Color: Green (rgb(76, 175, 80))
  - Stroke width: 2px
  - Class: 'simlin-target-good'
- **Invalid target**:
  - Color: Red (rgb(244, 67, 54))
  - Stroke width: 2px
  - Class: 'simlin-target-bad'

### Warning Indicators

Elements with errors/warnings display:
- **Appearance**: Orange circle (rgb(255, 152, 0))
- **Size**: 3px radius
- **Position**:
  - Circles (Aux/Flow): 45° angle (-π/4 radians) from center
  - Rectangles (Stock): Top-right corner (x + width/2 - 1, y - height/2 + 1)
- **No stroke**: fill only (stroke-width: 0)

### Sparklines

Mini time-series visualizations within elements:
- **Supported by**: Stocks, Aux, Flows, Aliases
- **Position**: Inset 1px from element bounds
- **Size**: Element dimension - 2px padding
- **Array adjustment**: Position accounts for array offset
- **SVG group**: Transformed to correct position
- **Data**: Array of Series objects with time/value pairs

## Constants and Dimensions

### Element Dimensions (pixels)
```typescript
// Basic shapes
const AuxRadius = 9;              // Auxiliary and flow valve radius
const StockWidth = 45;
const StockHeight = 35;
const ModuleWidth = 55;
const ModuleHeight = 45;

// Derived dimensions
const CloudRadius = 13.5;        // 1.5 × AuxRadius
const CloudWidth = 55;           // Original cloud SVG path width

// Visual details
const ArrowheadRadius = 6;       // Connector arrowheads
const FlowArrowheadRadius = 8;   // Flow arrowheads (larger)
const ModuleRadius = 5;          // Corner rounding for modules
const LabelPadding = 3;          // Space between element and label
const lineSpacing = 14;          // Vertical space between text lines

// Behavioral constants
const StraightLineMax = 6;       // Degrees - threshold for straight connectors
const finalAdjust = 7.5;         // Flow path endpoint adjustment for arrowhead
```

### Effective Radii for Connectors
Used for calculating intersection points:
```typescript
// Element-specific effective radii
const StockEffectiveRadius = 15;
const ModuleEffectiveRadius = 25;
const AuxEffectiveRadius = AuxRadius; // 9
const CloudEffectiveRadius = CloudRadius; // 13.5
const ZeroRadiusEffective = 0;
```

### Styling Constants
```typescript
// Stroke widths
const normalStroke = 1;          // Default element stroke
const connectorStroke = 0.5;     // Normal connectors
const selectedConnectorStroke = 1; // Selected connectors
const targetStroke = 2;          // Validation feedback
const flowOuterStroke = 4;       // Flow pipe outer
const flowInnerStroke = 2;       // Flow pipe inner
const cloudStroke = 2;           // Cloud outline

// Colors
const normalColor = 'black';
const selectedColor = '#4444dd';
const backgroundColor = 'white';
const warningColor = 'rgb(255, 152, 0)';
const validColor = 'rgb(76, 175, 80)';
const invalidColor = 'rgb(244, 67, 54)';
const connectorColor = 'gray';
const cloudColorLight = '#6388dc';
const cloudColorDark = '#2D498A';

// Dash arrays
const aliasDashArray = 2;        // Dashed stroke for aliases
```

## Rendering Architecture

### Component Hierarchy
```
Canvas (main SVG container)
├── Background grid (optional)
├── Connectors (rendered first, behind elements)
├── Flows (pipes and valves)
├── Clouds (attached to flows)
├── Stocks (rectangles)
├── Auxiliaries (circles)
├── Aliases (dashed circles)
├── Modules (rounded rectangles)
├── Labels (text elements)
├── Selection indicators
└── Warning indicators
```

### Rendering Order
1. **Background elements**: Grid, guides
2. **Connectors**: Drawn first so they appear behind other elements
3. **Flow pipes**: Behind flow valves
4. **Main elements**: Stocks, Auxiliaries, Modules
5. **Flow valves**: On top of pipes
6. **Clouds**: With their associated flows
7. **Labels**: On top of elements
8. **Overlays**: Selection highlights, warnings, sparklines

### SVG Structure
- **Main SVG**: Scaled by zoom factor using transform
- **Groups (g)**: Each element type wrapped in group for styling
- **CSS classes**: Used for theming and state (selected, warning, etc.)
- **Pointer events**: Handled at element level for interaction

## Hit Testing and Selection

### Hit Detection
Each element type has specific hit testing:
- **Circles** (Aux, Alias, Flow valve): Distance from center ≤ radius
- **Rectangles** (Stock, Module): Point within bounds
- **Clouds**: Distance from center ≤ CloudRadius
- **Connectors**: Invisible wider path for easier selection
- **Flows**: Both valve and path are selectable

### Selection Areas
- **Primary element**: The visible shape
- **Labels**: Separate hit target, double-click to edit
- **Extended hit area**: Background paths for thin elements
- **Arrowheads**: Separate selection target for reconnection

### Interaction Modes
- **Single click**: Select element
- **Double click on label**: Enter text edit mode
- **Drag element**: Move with constraints
- **Drag arrowhead**: Reconnect to different target
- **Drag label**: Reposition label side

## Best Practices for View Generation

### Element Positioning
1. **Spacing**: Minimum 20px between element edges
2. **Grid alignment**: Snap to 5px or 10px grid for cleaner layouts
3. **Label clearance**: Account for label bounds when positioning
4. **Flow clearance**: Keep valve at least 20px from connected elements

### Layout Strategies
1. **Hierarchical**: Arrange in levels (sources → stocks → sinks)
2. **Circular**: Place around feedback loops
3. **Grid**: Align to regular grid pattern
4. **Force-directed**: Use physics simulation for organic layout

### Naming Conventions
1. **Canonical names**: Use underscores for word separation
2. **Display names**: Automatically convert underscores to spaces
3. **Line breaks**: Use \n for multi-line labels
4. **Length limits**: Keep under 30 characters per line

### UID Management
1. **Sequential assignment**: Start from 0, increment by 1
2. **Uniqueness**: Required within view
3. **Persistence**: Maintain UIDs when modifying view
4. **References**: Always validate referenced UIDs exist

### Performance Considerations
1. **Element count**: Optimize for < 100 elements per view
2. **Connector complexity**: Prefer straight lines when possible
3. **Label rendering**: Cache text measurements
4. **Sparkline data**: Limit to necessary time points

## Example View Generation

### Complete Stock-Flow System
```typescript
const view = {
  nextUid: 8,
  zoom: 1.0,
  viewBox: { top: 50, left: 50, right: 550, bottom: 350 },
  elements: [
    // Central stock
    new StockViewElement({
      uid: 0,
      name: "inventory",
      ident: "inventory",
      x: 300,
      y: 200,
      labelSide: 'bottom',
      isZeroRadius: false,
      inflows: List([1]),
      outflows: List([2])
    }),

    // Production flow (inflow)
    new FlowViewElement({
      uid: 1,
      name: "production_rate",
      ident: "production_rate",
      x: 150,  // Valve position
      y: 200,
      labelSide: 'top',
      points: List([
        new Point({ x: 75, y: 200, attachedToUid: 3 }),   // From cloud
        new Point({ x: 278, y: 200, attachedToUid: 0 })   // To stock left edge (300 - 45/2)
      ]),
      isZeroRadius: false
    }),

    // Sales flow (outflow)
    new FlowViewElement({
      uid: 2,
      name: "sales_rate",
      ident: "sales_rate",
      x: 450,  // Valve position
      y: 200,
      labelSide: 'top',
      points: List([
        new Point({ x: 323, y: 200, attachedToUid: 0 }),  // From stock right edge (300 + 45/2)
        new Point({ x: 525, y: 200, attachedToUid: 4 })   // To cloud
      ]),
      isZeroRadius: false
    }),

    // Source cloud
    new CloudViewElement({
      uid: 3,
      flowUid: 1,
      x: 75,
      y: 200,
      isZeroRadius: false
    }),

    // Sink cloud
    new CloudViewElement({
      uid: 4,
      flowUid: 2,
      x: 525,
      y: 200,
      isZeroRadius: false
    }),

    // Demand auxiliary
    new AuxViewElement({
      uid: 5,
      name: "customer_demand",
      ident: "customer_demand",
      x: 450,
      y: 100,
      labelSide: 'right',
      isZeroRadius: false
    }),

    // Production capacity
    new AuxViewElement({
      uid: 6,
      name: "production_capacity",
      ident: "production_capacity",
      x: 150,
      y: 100,
      labelSide: 'left',
      isZeroRadius: false
    }),

    // Link from demand to sales
    new LinkViewElement({
      uid: 7,
      fromUid: 5,
      toUid: 2,
      arc: undefined,  // Straight line
      isStraight: true
    }),

    // Link from capacity to production
    new LinkViewElement({
      uid: 8,
      fromUid: 6,
      toUid: 1,
      arc: undefined,
      isStraight: true
    })
  ]
};
```

### Key Generation Guidelines

1. **Flow point calculation**:
   - Stock edges: ±StockWidth/2 (22.5) or ±StockHeight/2 (17.5)
   - Must attach points with attachedToUid
   - Cloud endpoints at CloudRadius from center

2. **Connector properties**:
   - Use `arc: undefined` and `isStraight: true` for straight lines
   - Specify arc in radians for curved connectors
   - Arc angle is takeoff angle from source

3. **Label positioning**:
   - Flows typically use 'top' or 'bottom'
   - Auxiliaries often use 'left' or 'right'
   - Stocks can use 'center' for overlay

4. **UID references**:
   - Flows reference stocks/clouds in points
   - Clouds reference their flow
   - Links reference from/to elements

This comprehensive documentation provides the complete specification for generating and understanding system dynamics diagram layouts in Simlin. Use these guidelines to create valid, well-formatted views that render correctly and provide good user experience.