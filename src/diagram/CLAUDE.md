# @simlin/diagram

Last verified: 2026-04-08

React components for model visualization and editing. Designed as a general-purpose SD model editor toolkit without dependencies on the Simlin app or server API.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).

## Key Files

### Editor and Core Logic

- `Editor.tsx` -- Main model editor: user interaction, state, and tool selection. Manages module navigation stack (`modelStack`), module CRUD handlers, and delegates to `ModuleDetails` for module editing.
- `VariableDetails.tsx` -- Variable properties/equation panel (stocks, flows, auxes)
- `ModuleDetails.tsx` -- Module properties panel: model reference selector, input wiring table, output ports, units/docs editors
- `BreadcrumbBar.tsx` -- Breadcrumb navigation: back arrow + breadcrumb trail when inside a module, hamburger menu at root
- `ModuleIcon.tsx` -- Module tool icon for the SpeedDial toolbar
- `group-movement.ts` -- Group manipulation and movement logic
- `selection-logic.ts` -- Selection state management
- `view-conversion.ts` -- View coordinate conversions
- `arc-utils.ts` -- Arc geometry helpers (`radToDeg`, `degToRad`, arc math)
- `keyboard-shortcuts.ts` -- Keyboard shortcut handling
- `StaticDiagram.tsx` -- Static (non-interactive) diagram renderer
- `HostedWebEditor.tsx` -- Web editor wrapper
- `LineChart.tsx` -- Chart visualization

### Module Logic (Functional Core)

- `module-navigation.ts` -- Module stack types (`ModuleStackEntry`, `NavigateResult`, `BreadcrumbSegment`) and pure functions (`pushModule`, `popModule`, `navigateToLevel`, `breadcrumbSegments`, `currentModelName`). Level 0 = main (root).
- `module-details-utils.ts` -- Module detail utilities: `countModelInstances`, `wouldCreateCycle` (DFS cycle detection), `getAvailableModels`, `getInputPorts`, `getPublicVariables`
- `module-wiring.ts` -- Module reference array manipulation: `addReference`, `removeReference`, `updateReferenceSrc`, `updateReferenceDst`, `getAvailableSrcVariables`
- `module-warning.ts` -- `anyModuleHasModelReference`: suppresses warning dots when no modules have references yet (new model sketching scenario)

### Drawing (`drawing/`)

- `Canvas.tsx` -- Main canvas and rendering engine. Supports module creation tool (`selectedTool: 'module'`), double-click drill-in on modules (`onDrillIntoModule`), and module warning suppression.
- `Flow.tsx` -- Flow/arc visual rendering
- `Connector.tsx` -- Connection/link rendering and arc geometry (`computeLinkCreationArc`)
- `Stock.tsx` -- Stock visualization
- `Auxiliary.tsx` -- Auxiliary variable rendering
- `Label.tsx` -- Text label rendering
- `Cloud.tsx` -- Cloud/source-sink rendering
- `Module.tsx` -- Module visualization. Supports `onDoubleClick` callback for drill-in navigation.
- `Alias.tsx` -- Alias (ghost) rendering
- `Sparkline.tsx` -- Inline sparkline charts

### UI Components (`components/`)

Material-style UI component library (40+ components): Accordion, AppBar, Button, Card, Dialog, Drawer, etc.

## Invariants

- **Optimistic view updates**: `updateView()` and `queueViewUpdate()` in Editor.tsx call `setView(view)` + increment `projectVersion` synchronously before awaiting the engine round-trip. Any new handler that modifies the view must go through these methods to avoid flicker.
- **Link drag arc ownership**: During any single-link arrowhead drag (creation or reattachment), Canvas.tsx's `connector()` has exclusive control over the arc. `applyGroupMovement` is intentionally given `arcPoint: undefined` during link drags so `processLinks` does not interfere. The last-rendered arc is cached in `draggedLinkArc` and used at mouse-up for exact visual consistency.
- **Collinear defense in Connector geometry**: `takeoffθ()` and `arcCircle()` catch `circleFromPoints` throws for collinear points (cursor on the source-to-target line). Any code passing user cursor positions as arc points must handle this gracefully or go through these functions.
- **Module navigation stack**: `Editor.modelStack` is an immutable array of `ModuleStackEntry`. Each entry stores the child model name, module ident, and the parent's selection/viewBox/zoom for restoration. `currentModelName(stack)` returns the active model. All navigation goes through `pushModule`/`popModule`/`navigateToLevel` pure functions.
- **Module patches target `modelName`**: Module creation and editing patches use `this.state.modelName` (not hardcoded 'main'), so operations work at any nesting depth.
- **Module warning suppression**: When no module in a model has a model reference, warning indicators are suppressed on all modules. This prevents a wall of warnings during initial module layout.

## Gotchas

- **buildSelectionMap async race**: When `inCreation` is undefined but selection still references `inCreationUid`, the entry is silently skipped. This handles the transient state between Canvas clearing `inCreation` and Editor's async handler updating selection.
- **Touch links are always straight**: When `dragPointerType === 'touch'`, link creation always produces `arc: undefined` (straight line) because touch interactions lack a stable cursor midpoint.
- **Module upsert is full replacement**: The engine does full variable replacement on `upsertModule`, not merge. All module handlers in Editor.tsx must send the complete module state (modelName, references, units, documentation) in every patch.
- **projectOps ordering**: `AddModel` in `projectOps` is processed before model-level `ops`, allowing atomic create-and-reference in a single patch. `handleCreateModelForModule` relies on this ordering.

## Additional Documentation

- `LAYOUT.md` -- Comprehensive layout and architecture documentation for the diagram package
