# @simlin/diagram

React components for model visualization and editing. Designed as a general-purpose SD model editor toolkit without dependencies on the Simlin app or server API.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [doc/dev/commands.md](/doc/dev/commands.md).

## Key Files

### Editor and Core Logic

- `Editor.tsx` -- Main model editor: user interaction, state, and tool selection
- `VariableDetails.tsx` -- Variable properties/equation panel
- `group-movement.ts` -- Group manipulation and movement logic
- `selection-logic.ts` -- Selection state management
- `view-conversion.ts` -- View coordinate conversions
- `keyboard-shortcuts.ts` -- Keyboard shortcut handling
- `StaticDiagram.tsx` -- Static (non-interactive) diagram renderer
- `HostedWebEditor.tsx` -- Web editor wrapper
- `LineChart.tsx` -- Chart visualization

### Drawing (`drawing/`)

- `Canvas.tsx` -- Main canvas and rendering engine
- `Flow.tsx` -- Flow/arc visual rendering
- `Connector.tsx` -- Connection/link rendering
- `Stock.tsx` -- Stock visualization
- `Auxiliary.tsx` -- Auxiliary variable rendering
- `Label.tsx` -- Text label rendering
- `Cloud.tsx` -- Cloud/source-sink rendering
- `Module.tsx` -- Module visualization
- `Alias.tsx` -- Alias (ghost) rendering
- `Sparkline.tsx` -- Inline sparkline charts

### UI Components (`components/`)

Material-style UI component library (40+ components): Accordion, AppBar, Button, Card, Dialog, Drawer, etc.

## Additional Documentation

- `LAYOUT.md` -- Comprehensive layout and architecture documentation for the diagram package
