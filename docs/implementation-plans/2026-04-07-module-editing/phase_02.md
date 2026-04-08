# Module Editing Implementation Plan -- Phase 2: Module Creation and TypeScript Patch Types

**Goal:** Toolbar button for creating modules on the diagram, plus TypeScript patch type additions to persist them (Rust patch types already exist)

**Architecture:** Add `'module'` to the `selectedTool` union, create a `ModuleIcon` SpeedDial action, extend `Canvas.handlePointerDown` with a module creation branch that follows the same immediate-placement pattern as stock/aux. Extend `Editor.handleCreateVariable` to emit `upsertModule` patches (already supported by the engine). Add `AddModel` to the TypeScript `JsonProjectOperation` type. Add warning indicator logic for unconfigured modules.

**Tech Stack:** React, TypeScript, CSS Modules, Rust (patch types already done)

**Scope:** 4 phases from original design (phase 2 of 4)

**Codebase verified:** 2026-04-07

**Testing references:**
- Testing patterns: `src/diagram/CLAUDE.md`, `docs/dev/typescript.md`
- Jest config: `src/diagram/jest.config.js` (jsdom environment, tests in `tests/` subdirectory)
- Preferred pattern: extract pure functions, test directly
- Existing module test file: `src/diagram/tests/module-interaction.test.ts` (tests `moduleContains`, `moduleBounds`)

**Key finding from codebase investigation:** `upsertModule` already exists in both TypeScript (`src/engine/src/json-types.ts` line 476) and Rust (`src/libsimlin/src/patch.rs` line 114). `AddModel` is already supported by Rust JSON deserialization (`src/libsimlin/src/patch.rs` line 78) but NOT exposed in the TypeScript `JsonProjectOperation` type (line 528). No new Rust code is needed.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### module-editing.AC1: Module Creation
- **module-editing.AC1.1 Success:** Module tool button appears in SpeedDial toolbar alongside stock/flow/aux/link
- **module-editing.AC1.2 Success:** Clicking canvas with module tool places a module immediately at click position (no dialog)
- **module-editing.AC1.3 Success:** Inline name editing begins after placement (same flow as stock/aux)
- **module-editing.AC1.4 Success:** Newly created module has no model reference and empty references array
- **module-editing.AC1.5 Success:** Module without a model reference shows warning indicator (error dot, same pattern as aux with missing equation)
- **module-editing.AC1.6 Edge:** Warning indicator is suppressed when no modules in the current model have model references (new model scenario)
- **module-editing.AC1.13 Failure:** Canceling name editing after placement removes the module (same as stock/aux cancellation)

### module-editing.AC5: Composable Foundation
- **module-editing.AC5.2 Success:** Module creation dialog is available at any nesting depth (creating modules inside modules)

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Create ModuleIcon component

**Verifies:** module-editing.AC1.1

**Files:**
- Create: `src/diagram/ModuleIcon.tsx`
- Create: `src/diagram/ModuleIcon.module.css`

**Implementation:**

Create a `ModuleIcon` functional component following the `StockIcon` pattern (`src/diagram/StockIcon.tsx`). Use `SvgIcon` from `./components/SvgIcon` with a rounded rectangle SVG shape matching the module's visual appearance.

The module is rendered as a rounded rectangle on the diagram (`ModuleRadius = 5` from `src/diagram/drawing/default.ts`). The icon should be a proportional rounded rectangle in a 50x50 viewBox:

```typescript
import * as React from 'react';
import SvgIcon from './components/SvgIcon';
import styles from './ModuleIcon.module.css';

export const ModuleIcon: React.FunctionComponent = (props) => {
  return (
    <SvgIcon viewBox="0 0 50 50" className={styles.moduleIcon} {...props}>
      <g>
        <rect x={2.5} y={7.5} width={45} height={35} rx={5} ry={5} />
      </g>
    </SvgIcon>
  );
};

ModuleIcon.displayName = 'Module';
```

CSS file:
```css
.moduleIcon {
  fill: gray;
}
```

The key visual distinction from `StockIcon` is the rounded corners (`rx`/`ry` attributes).

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: add ModuleIcon component for SpeedDial toolbar`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Add module tool to SpeedDial toolbar and Editor state

**Verifies:** module-editing.AC1.1

**Files:**
- Modify: `src/diagram/Editor.tsx` (EditorState interface at line ~178, getEditorControls at line ~2250, add handleSelectModule handler)

**Implementation:**

**EditorState change** -- extend `selectedTool` union (line ~178):

```typescript
selectedTool: 'stock' | 'flow' | 'aux' | 'link' | 'module' | undefined;
```

**Add handler** -- following the pattern of `handleSelectStock` (line ~1941):

```typescript
handleSelectModule = (e: React.MouseEvent<HTMLButtonElement>) => {
  e.preventDefault();
  e.stopPropagation();
  this.setState({
    selectedTool: 'module',
  });
};
```

**Add SpeedDialAction** -- in `getEditorControls()` (after the Link action at line ~2291), add:

```tsx
<SpeedDialAction
  icon={<ModuleIcon />}
  title="Module"
  onClick={this.handleSelectModule}
  selected={selectedTool === 'module'}
/>
```

Import `ModuleIcon` from `'./ModuleIcon'` at the top of the file.

**CanvasProps change** -- update `selectedTool` in `CanvasProps` (at `src/diagram/drawing/Canvas.tsx` line ~163):

```typescript
selectedTool: 'stock' | 'flow' | 'aux' | 'link' | 'module' | undefined;
```

Also update the `prevSelectedTool` field on the Canvas class (line ~196) to include `'module'`.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: add module tool to SpeedDial and selectedTool union`

<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Add module creation flow in Canvas.handlePointerDown

**Verifies:** module-editing.AC1.2, module-editing.AC1.3, module-editing.AC1.13

**Files:**
- Modify: `src/diagram/drawing/Canvas.tsx` (handlePointerDown at line ~1741)

**Implementation:**

In `handlePointerDown`, the existing code at line ~1741 checks `selectedTool === 'aux' || selectedTool === 'stock'`. Add `selectedTool === 'module'` to this condition and add a module creation branch.

The module creation follows the exact same flow as aux/stock: create a `ModuleViewElement` with `inCreationUid`, set `editNameOnPointerUp: true`, and the existing name editing flow handles the rest.

Inside the `if (selectedTool === 'aux' || selectedTool === 'stock')` block (or extending it to also handle `'module'`), add a third branch:

```typescript
if (selectedTool === 'aux' || selectedTool === 'stock' || selectedTool === 'module') {
  let inCreation: AuxViewElement | StockViewElement | ModuleViewElement;
  if (selectedTool === 'aux') {
    // ... existing aux creation code (lines 1743-1755) ...
  } else if (selectedTool === 'stock') {
    // ... existing stock creation code (lines 1757-1770) ...
  } else {
    // Module creation - same pattern as aux but with 'module' type
    const name = this.getNewVariableName('New Module');
    inCreation = {
      type: 'module',
      uid: inCreationUid,
      var: undefined,
      x: client.x - canvasOffset.x,
      y: client.y - canvasOffset.y,
      name,
      ident: canonicalize(name),
      labelSide: 'bottom',
      isZeroRadius: false,
    };
  }
  // ... rest of existing creation flow (pointer capture, setState, onSetSelection) ...
}
```

Import `ModuleViewElement` from `@simlin/core/datamodel` if not already imported.

Note: `ModuleViewElement` (core/datamodel.ts line 634) does include `isZeroRadius: boolean` as a required field, so setting it to `false` in the creation code is correct.

AC1.3 (inline name editing) is automatically satisfied by the existing flow: `editNameOnPointerUp: true` triggers name editing on pointer up.

AC1.13 (cancellation removes module) is automatically satisfied by the existing flow: `handleEditingNameCancel` removes elements with `inCreationUid`.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: add module creation branch in Canvas.handlePointerDown`

<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Add module branch in Editor.handleCreateVariable

**Verifies:** module-editing.AC1.4

**Files:**
- Modify: `src/diagram/Editor.tsx` (handleCreateVariable at line ~1103)

**Implementation:**

In `handleCreateVariable` (line ~1103), the current code dispatches `upsertStock`, `upsertFlow`, or falls through to `upsertAux` based on `elementType`. Add a `'module'` branch before the `else` catch-all.

The `upsertModule` operation already exists in `JsonModelOperation` (see `src/engine/src/json-types.ts` line 476). The patch payload uses `JsonModule` which requires `name` and `modelName`.

```typescript
let op: JsonModelOperation;
if (elementType === 'stock') {
  op = { type: 'upsertStock', payload: { stock: { name, inflows: [], outflows: [], initialEquation: '' } } };
} else if (elementType === 'flow') {
  op = { type: 'upsertFlow', payload: { flow: { name, equation: '' } } };
} else if (elementType === 'module') {
  op = {
    type: 'upsertModule',
    payload: {
      module: {
        name,
        modelName: '',     // No model reference initially (AC1.4)
        references: [],     // Empty references array (AC1.4)
      },
    },
  };
} else {
  op = { type: 'upsertAux', payload: { aux: { name, equation: '' } } };
}
```

This ensures newly created modules have `modelName: ''` (no model reference) and `references: []` (empty), satisfying AC1.4.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: add module branch in handleCreateVariable for upsertModule patch`

<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-6) -->

<!-- START_TASK_5 -->
### Task 5: Add AddModel to TypeScript JsonProjectOperation

**Verifies:** None (infrastructure for Phase 3)

**Files:**
- Modify: `src/engine/src/json-types.ts` (JsonProjectOperation at line ~528)

**Implementation:**

The Rust JSON deserialization layer (`src/libsimlin/src/patch.rs` line 78) already handles `AddModel` in `JsonProjectOperation`. Add the matching TypeScript type.

Add `AddModelOp` type and payload (near the other operation types around line ~458):

```typescript
/**
 * Payload for add model operation.
 */
export interface AddModelPayload {
  name: string;
}

/**
 * Add a new model to the project.
 */
export interface AddModelOp {
  type: 'addModel';
  payload: AddModelPayload;
}
```

Update the `JsonProjectOperation` union (line ~528):

```typescript
export type JsonProjectOperation = SetSimSpecsOp | AddModelOp;
```

Add a type guard following the existing pattern:

```typescript
export function isAddModel(op: JsonProjectOperation): op is AddModelOp {
  return op.type === 'addModel';
}
```

**Verification:**
Run: `cd src/diagram && npx jest && cd ../engine && npx jest`
Expected: All existing tests pass (type-only change)

**Commit:** `engine: add AddModel to TypeScript JsonProjectOperation type`

<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Module warning indicator with suppression logic

**Verifies:** module-editing.AC1.5, module-editing.AC1.6

**Files:**
- Modify: `src/diagram/drawing/Canvas.tsx` (module() method at line ~503)
- Create or modify: `src/diagram/module-warning.ts` (new pure function for testability)

**Implementation:**

The warning indicator infrastructure already exists in `Module.tsx` (`hasWarning` prop, `indicators()` method rendering the error dot at lines 75-87). The engine reports errors for modules with empty `modelName`. The `variableHasError()` function in `src/core/datamodel.ts` (line 353) checks for these engine-reported errors.

The new logic needed is the **suppression** for AC1.6: when NO modules in the current model have model references, suppress warnings for all modules. This prevents a wall of warning dots when a user is rapidly sketching module structure before configuring model references.

Create a pure function for testability:

```typescript
// src/diagram/module-warning.ts
import { type Variable } from '@simlin/core/datamodel';

/**
 * Returns true if any module variable in the given variables map has a non-empty modelName.
 * Used to determine whether to show warning indicators on unconfigured modules.
 * When no modules have model references yet (new model scenario), warnings are suppressed.
 */
export function anyModuleHasModelReference(variables: ReadonlyMap<string, Variable>): boolean {
  for (const variable of variables.values()) {
    if (variable.type === 'module' && variable.modelName !== '') {
      return true;
    }
  }
  return false;
}
```

In `Canvas.tsx`, compute this once at the start of `buildLayers()` (or as a lazy cached property) and use it in the `module()` method:

```typescript
// In buildLayers() or at the top of render():
this.hasAnyModuleReference = anyModuleHasModelReference(this.props.model.variables);

// In module() method (line ~503):
module(element: ModuleViewElement) {
  const variable = this.props.model.variables.get(element.ident);
  const hasEngineError = variable ? variableHasError(variable) : false;
  // AC1.6: suppress if no module in model has a model reference
  const hasWarning = hasEngineError && this.hasAnyModuleReference;
  // ... rest unchanged ...
}
```

Add `hasAnyModuleReference` as a class field on Canvas (initialized to `false`), updated each render cycle.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: add module warning indicator with suppression logic`

<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_C -->

<!-- START_SUBCOMPONENT_D (tasks 7-8) -->

<!-- START_TASK_7 -->
### Task 7: Tests for module creation and warning logic

**Verifies:** module-editing.AC1.1, module-editing.AC1.2, module-editing.AC1.4, module-editing.AC1.5, module-editing.AC1.6, module-editing.AC1.13

**Files:**
- Create: `src/diagram/tests/module-creation.test.ts`

**Testing:**

Test the pure functions and creation logic. Use `@jest-environment node` since no DOM is needed for pure function tests.

Test cases for `anyModuleHasModelReference`:

- **module-editing.AC1.6 (suppression):** Empty variables map returns false
- **module-editing.AC1.6 (suppression):** Map with only non-module variables returns false
- **module-editing.AC1.6 (suppression):** Map with one module, empty modelName returns false
- **module-editing.AC1.6 (suppression):** Map with two modules, both empty modelName returns false
- **module-editing.AC1.5 (warning shown):** Map with one module having modelName='hares' returns true
- **module-editing.AC1.5 (warning shown):** Map with two modules, one configured one not, returns true

Test cases for module creation flow (unit tests for the data structures):

- **module-editing.AC1.2 (placement):** Creating a `ModuleViewElement` with type 'module' at a specific position produces correct coordinates
- **module-editing.AC1.4 (no model ref):** The `upsertModule` payload has `modelName: ''` and `references: []`
- **module-editing.AC1.1 (tool selection):** The selectedTool union includes 'module' (compile-time check via TypeScript, but verify the handler pattern matches other tools)

- **module-editing.AC5.2 (creation at any depth):** The module creation flow uses `this.state.modelName` for the patch target model. When navigated into a child model (non-empty modelStack), `modelName` reflects that child model. Verify that creating a module at depth > 1 dispatches the `upsertModule` patch targeting the inner model, not 'main'. Test by constructing state with a non-empty modelStack and verifying the patch model name matches the current (inner) modelName.

**Verification:**
Run: `cd src/diagram && npx jest tests/module-creation.test.ts`
Expected: All tests pass

**Commit:** `diagram: add tests for module creation and warning logic`

<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Integration test for module creation with engine

**Verifies:** module-editing.AC1.4, module-editing.AC1.13

**Files:**
- Modify: `src/diagram/tests/editor-applyPatch.test.ts` or create: `src/diagram/tests/module-patch.test.ts`

**Testing:**

Test that the `upsertModule` patch operation works end-to-end with the WASM engine. Follow the pattern in `src/diagram/tests/editor-applyPatch.test.ts` (which loads the real WASM engine and applies patches).

Use `@jest-environment node` docblock. Load WASM via:
```typescript
import { configureWasm, ready, type EngineProject } from '@simlin/engine';
import * as fs from 'fs';
const wasmBuffer = fs.readFileSync('src/engine/core/libsimlin.wasm');
```

Test cases:

- **module-editing.AC1.4:** Create a project, apply `upsertModule` patch with `name: 'my_module', modelName: '', references: []`. Verify the module appears in the model's variables with correct fields.

- **module-editing.AC1.4:** Apply `upsertModule` then serialize and deserialize. Verify roundtrip preserves the module.

- Verify `addModel` project operation creates a new empty model and that a module can reference it.

**Verification:**
Run: `cd src/diagram && npx jest tests/module-patch.test.ts`
Expected: All tests pass

**Commit:** `diagram: add integration test for module upsertModule patch`

<!-- END_TASK_8 -->

<!-- END_SUBCOMPONENT_D -->
