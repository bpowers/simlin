# @simlin/diagram

Last verified: 2026-03-28

React components for model visualization and editing. Designed as a general-purpose SD model editor toolkit without dependencies on the Simlin app or server API.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).

## Key Files

### Editor and Core Logic

- `Editor.tsx` -- Main model editor: user interaction, state, and tool selection
- `VariableDetails.tsx` -- Variable properties/equation panel
- `group-movement.ts` -- Group manipulation and movement logic
- `selection-logic.ts` -- Selection state management
- `view-conversion.ts` -- View coordinate conversions
- `arc-utils.ts` -- Arc geometry helpers (`radToDeg`, `degToRad`, arc math)
- `keyboard-shortcuts.ts` -- Keyboard shortcut handling
- `StaticDiagram.tsx` -- Static (non-interactive) diagram renderer
- `HostedWebEditor.tsx` -- Web editor wrapper
- `LineChart.tsx` -- Chart visualization

### Drawing (`drawing/`)

- `Canvas.tsx` -- Main canvas and rendering engine
- `Flow.tsx` -- Flow/arc visual rendering
- `Connector.tsx` -- Connection/link rendering and arc geometry (`computeLinkCreationArc`)
- `Stock.tsx` -- Stock visualization
- `Auxiliary.tsx` -- Auxiliary variable rendering
- `Label.tsx` -- Text label rendering
- `Cloud.tsx` -- Cloud/source-sink rendering
- `Module.tsx` -- Module visualization
- `Alias.tsx` -- Alias (ghost) rendering
- `Sparkline.tsx` -- Inline sparkline charts

### UI Components (`components/`)

Material-style UI component library (40+ components): Accordion, AppBar, Button, Card, Dialog, Drawer, etc.

## Invariants

- **Optimistic view updates**: `updateView()` and `queueViewUpdate()` in Editor.tsx call `setView(view)` + increment `projectVersion` synchronously before awaiting the engine round-trip. Any new handler that modifies the view must go through these methods to avoid flicker.
- **Link drag arc ownership**: During any single-link arrowhead drag (creation or reattachment), Canvas.tsx's `connector()` has exclusive control over the arc. `applyGroupMovement` is intentionally given `arcPoint: undefined` during link drags so `processLinks` does not interfere. The last-rendered arc is cached in `draggedLinkArc` and used at mouse-up for exact visual consistency.
- **Collinear defense in Connector geometry**: `takeoffθ()` and `arcCircle()` catch `circleFromPoints` throws for collinear points (cursor on the source-to-target line). Any code passing user cursor positions as arc points must handle this gracefully or go through these functions.

## Gotchas

- **buildSelectionMap async race**: When `inCreation` is undefined but selection still references `inCreationUid`, the entry is silently skipped. This handles the transient state between Canvas clearing `inCreation` and Editor's async handler updating selection.
- **Touch links are always straight**: When `dragPointerType === 'touch'`, link creation always produces `arc: undefined` (straight line) because touch interactions lack a stable cursor midpoint.

## Additional Documentation

- `LAYOUT.md` -- Comprehensive layout and architecture documentation for the diagram package
