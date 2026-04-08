# Module Editing Implementation Plan -- Phase 3: Module Details Panel

**Goal:** Dedicated details panel for modules showing model reference selection, input wiring (read-only), output ports, shared model awareness, and units/documentation editing

**Architecture:** Create a `ModuleDetails` component that renders in the same slot as `VariableDetails`, activated when the selected variable has `type === 'module'`. Add `canBeModuleInput` and `isPublic` fields to core Variable types so ModuleDetails can identify input ports and public variables in the referenced model. Add utility functions for cycle detection and model instance counting. Add a shared model banner to the Editor.

**Tech Stack:** React, TypeScript, Slate (for units/docs editors), CSS Modules

**Scope:** 4 phases from original design (phase 3 of 4)

**Codebase verified:** 2026-04-07

**Testing references:**
- Testing patterns: `src/diagram/CLAUDE.md`, `docs/dev/typescript.md`
- Jest config: `src/diagram/jest.config.js` (jsdom environment, tests in `tests/` subdirectory)
- Preferred pattern: extract pure functions, test directly
- VariableDetails pattern: `src/diagram/VariableDetails.tsx` (Slate editors, Tab component, delete button)
- Dialog component: `src/diagram/components/Dialog.tsx` (radix-ui wrapper)

**Key finding from codebase investigation:** `canBeModuleInput` and `isPublic` exist in `JsonCompat` (`src/engine/src/json-types.ts` line 36) and are handled by the Rust engine, but are NOT propagated to `src/core/datamodel.ts` Variable types. The core `Aux`, `Stock`, `Flow` interfaces lack these fields; `auxFromJson()`, `stockFromJson()`, `flowFromJson()` strip them during deserialization. Phase 3 must add these fields to the core data model to enable the details panel to identify input ports and public variables.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### module-editing.AC2: Module Details Panel
- **module-editing.AC2.1 Success:** Selecting a module shows ModuleDetails panel (not VariableDetails)
- **module-editing.AC2.2 Success:** Panel displays the referenced model name
- **module-editing.AC2.3 Success:** Panel lists all input references with src -> dst mapping
- **module-editing.AC2.4 Success:** Panel lists all output ports (public variables) of the referenced model
- **module-editing.AC2.7 Success:** Units and documentation are editable on modules
- **module-editing.AC2.8 Success:** "Open Model" button navigates into the module's model
- **module-editing.AC2.9 Edge:** Module referencing a model with zero input ports shows empty input wiring section (no errors)
- **module-editing.AC2.10 Edge:** Module referencing a model with zero public outputs shows empty output ports section

### module-editing.AC1: Module Creation (model reference selection)
- **module-editing.AC1.7 Success:** Model reference is selected in the ModuleDetails panel (not at creation time)
- **module-editing.AC1.8 Success:** Selecting a project-defined model as reference clears the warning indicator
- **module-editing.AC1.9 Success:** Selecting a stdlib model as reference works
- **module-editing.AC1.10 Success:** "Create new model" in ModuleDetails creates an empty model and sets it as the reference
- **module-editing.AC1.11 Success:** "Duplicate model" in ModuleDetails creates a copy and sets it as the reference
- **module-editing.AC1.12 Failure:** Models that would create circular nesting are excluded from the model selector

### module-editing.AC4: Shared Model Editing Awareness
- **module-editing.AC4.1 Success:** When viewing a model used by N>1 modules, a banner shows "This model is used by N modules -- changes affect all instances"
- **module-editing.AC4.2 Success:** Banner shows correct count after modules are added or removed
- **module-editing.AC4.3 Edge:** Model used by exactly 1 module shows no banner (or adapted message)
- **module-editing.AC4.4 Edge:** Stdlib models show "Standard library model (read-only)" instead of instance count

### module-editing.AC5: Composable Foundation
- **module-editing.AC5.3 Success:** ModuleDetails panel works correctly for modules at any nesting depth

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Add canBeModuleInput and isPublic to core Variable types

**Verifies:** module-editing.AC2.4 (output ports require isPublic), module-editing.AC2.9 (input ports require canBeModuleInput)

**Files:**
- Modify: `src/core/datamodel.ts` (Aux interface at line ~307, Stock interface at line ~278, Flow interface at line ~293, auxFromJson at line ~468, stockFromJson at line ~367, flowFromJson at line ~417, auxToJson at line ~484, stockToJson at line ~386, flowToJson at line ~436)

**Implementation:**

Add two optional boolean fields to each of the three variable types that can be input ports or public outputs:

```typescript
export interface Aux {
  // ... existing fields ...
  readonly canBeModuleInput: boolean;
  readonly isPublic: boolean;
}

export interface Stock {
  // ... existing fields ...
  readonly canBeModuleInput: boolean;
  readonly isPublic: boolean;
}

export interface Flow {
  // ... existing fields ...
  readonly canBeModuleInput: boolean;
  readonly isPublic: boolean;
}
```

Default both to `false` in the `*FromJson` functions, reading from `json.compat`:

In `auxFromJson` (line ~468), add to the returned object:
```typescript
canBeModuleInput: json.compat?.canBeModuleInput ?? false,
isPublic: json.compat?.isPublic ?? false,
```

Same pattern in `stockFromJson` and `flowFromJson`.

In the `*ToJson` functions, write back to `compat` if the values are non-default:

```typescript
if (aux.canBeModuleInput) {
  if (!result.compat) { result.compat = {}; }
  result.compat.canBeModuleInput = true;
}
if (aux.isPublic) {
  if (!result.compat) { result.compat = {}; }
  result.compat.isPublic = true;
}
```

Same pattern in `stockToJson` and `flowToJson`.

**Important: compat field preservation.** The existing `*ToJson` functions already set `result.compat` fields for `nonNegative` (see `stockToJson` at line ~405). The `if (!result.compat) { result.compat = {}; }` guard preserves any previously-set compat fields. Verify that adding `canBeModuleInput` and `isPublic` does not overwrite existing compat entries. Add a roundtrip test: serialize a variable with `nonNegative: true` AND `canBeModuleInput: true`, deserialize, and verify both fields survive.

**Verification:**
Run: `cd src/core && npx jest && cd ../diagram && npx jest`
Expected: All existing tests pass

**Commit:** `core: add canBeModuleInput and isPublic to Variable types`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Create module details utility functions

**Verifies:** module-editing.AC1.12, module-editing.AC4.1, module-editing.AC4.2, module-editing.AC4.3

**Files:**
- Create: `src/diagram/module-details-utils.ts`
- Create: `src/diagram/tests/module-details-utils.test.ts`

**Implementation:**

Create pure utility functions for the ModuleDetails panel:

1. **`countModelInstances(project, modelName)`** -- Count how many modules across all models reference a given model name. Iterate `project.models.values()`, for each model iterate `model.variables.values()`, filter `v.type === 'module'`, count where `v.modelName === modelName`. Returns a number.

2. **`getAvailableModels(project, currentModelName)`** -- Returns list of model names available for a module's model reference. Includes project models (excluding `currentModelName` and models that would create cycles) and stdlib model names. Returns `{ projectModels: string[], stdlibModels: string[] }`.

3. **`wouldCreateCycle(project, fromModelName, toModelName)`** -- Returns true if setting a module in `fromModelName` to reference `toModelName` would create a circular nesting. Performs DFS from `toModelName` following module.modelName references; if it reaches `fromModelName`, there's a cycle.

4. **`getInputPorts(model)`** -- Returns variables from a model where `canBeModuleInput === true`. These are the available `dst` targets for module references.

5. **`getPublicVariables(model)`** -- Returns variables from a model where `isPublic === true`. These are the output ports shown in the details panel.

**Testing:**

Tests in `src/diagram/tests/module-details-utils.test.ts` with `@jest-environment node`:

- **countModelInstances:** Zero modules returns 0; one module referencing 'hares' returns 1; two modules in different models referencing 'hares' returns 2; module referencing different model doesn't count
- **module-editing.AC4.3:** Model used by exactly 1 module returns 1
- **module-editing.AC1.12 (cycle detection):** A->B->C, adding C->A would create cycle (returns true); A->B, adding A->C doesn't create cycle (returns false); self-reference A->A returns true
- **getAvailableModels:** Excludes current model name; excludes models that would create cycles; includes stdlib models
- **getInputPorts:** Returns only variables with canBeModuleInput=true; empty when model has no input ports
- **getPublicVariables:** Returns only variables with isPublic=true; empty when model has no public variables

Build test fixtures using the `Project`, `Model`, `Variable` types from `@simlin/core/datamodel`. Create minimal models with modules referencing each other to test the graph traversal.

**Verification:**
Run: `cd src/diagram && npx jest tests/module-details-utils.test.ts`
Expected: All tests pass

**Commit:** `diagram: add module details utility functions with tests`

<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-5) -->

<!-- START_TASK_3 -->
### Task 3: Create ModuleDetails component -- model reference and metadata

**Verifies:** module-editing.AC2.1, module-editing.AC2.2, module-editing.AC1.7, module-editing.AC1.8, module-editing.AC1.9, module-editing.AC1.10, module-editing.AC1.11

**Files:**
- Create: `src/diagram/ModuleDetails.tsx`
- Create: `src/diagram/ModuleDetails.module.css`

**Implementation:**

Create a `ModuleDetails` React class component that renders in the same panel slot as `VariableDetails`. Follow the visual structure and styling patterns from `VariableDetails.tsx` and `VariableDetails.module.css`.

**Props interface:**

```typescript
interface ModuleDetailsProps {
  variable: Module;
  viewElement: ViewElement;
  project: Project;
  currentModelName: string;
  onDelete: (ident: string) => void;
  onModelReferenceChange: (ident: string, newModelName: string) => void;
  onUnitsDocsChange: (ident: string, newUnits: string | undefined, newDocs: string | undefined) => void;
  onDrillIntoModule: (moduleIdent: string, targetModelName: string) => void;
  onCreateModel: (moduleName: string) => void;
  onDuplicateModel: (moduleIdent: string, sourceModelName: string) => void;
}
```

**Component sections (top to bottom):**

1. **Module name header**: Display the module's `ident` as a title. Not editable here (renamed via inline editing on canvas, same as other variables).

2. **Model reference selector**: A dropdown/select showing available models. Use `getAvailableModels(project, currentModelName)` to populate. Sections:
   - Project-defined models (clickable to select)
   - Stdlib models (clickable to select)
   - "Create new model" action -- calls `onCreateModel(variable.ident)` which creates a new empty model via `AddModel` patch and sets it as the reference
   - "Duplicate existing model" action -- only shown when a model is already referenced; see `onDuplicateModel` prop below

   When no model is selected yet, show a prompt: "Select a model to instantiate". When a model is selected, show the model name as a link/button.

3. **"Open Model" button**: Visible when a model reference is set. Calls `onDrillIntoModule(variable.ident, variable.modelName)`. Uses the same navigation mechanism from Phase 1.

**CSS:** Follow the `.card` pattern from `VariableDetails.module.css`:
- `width: var(--panel-width-sm)` with responsive breakpoints
- `max-height: calc(100vh - 18px)` with `overflow-y: auto`
- Same box shadow and border radius

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: create ModuleDetails component with model reference selector`

<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Add input wiring table and output ports list to ModuleDetails

**Verifies:** module-editing.AC2.3, module-editing.AC2.4, module-editing.AC2.9, module-editing.AC2.10

**Files:**
- Modify: `src/diagram/ModuleDetails.tsx`
- Modify: `src/diagram/ModuleDetails.module.css`

**Implementation:**

Add two read-only sections to ModuleDetails, both visible only after a model reference is set:

**Input wiring table (AC2.3):**

Display the module's `references` array as a table with two columns: `src` (parent variable) -> `dst` (child input port). Each row shows one `ModuleReference`. If there are no references, show "No inputs configured" text (AC2.9).

```tsx
<div className={styles.section}>
  <div className={styles.sectionTitle}>Input Wiring</div>
  {variable.references.length === 0 ? (
    <div className={styles.emptyMessage}>No inputs configured</div>
  ) : (
    <table className={styles.wiringTable}>
      <thead>
        <tr><th>Source (parent)</th><th>Destination (module)</th></tr>
      </thead>
      <tbody>
        {variable.references.map((ref, i) => (
          <tr key={i}><td>{ref.src}</td><td>{ref.dst}</td></tr>
        ))}
      </tbody>
    </table>
  )}
</div>
```

**Output ports list (AC2.4):**

Get the referenced model from `project.models.get(variable.modelName)`, then call `getPublicVariables(referencedModel)` to find public variables. Display as a simple list showing variable idents. If there are none, show "No public outputs" text (AC2.10).

```tsx
<div className={styles.section}>
  <div className={styles.sectionTitle}>Output Ports</div>
  {publicVars.length === 0 ? (
    <div className={styles.emptyMessage}>No public outputs</div>
  ) : (
    <ul className={styles.portList}>
      {publicVars.map(v => (
        <li key={v.ident}>{v.ident}</li>
      ))}
    </ul>
  )}
</div>
```

**CSS for these sections:** Add styles for `.section`, `.sectionTitle`, `.wiringTable`, `.portList`, `.emptyMessage` to the CSS module.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: add read-only input wiring table and output ports to ModuleDetails`

<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Add units/docs editors and delete button to ModuleDetails

**Verifies:** module-editing.AC2.7

**Files:**
- Modify: `src/diagram/ModuleDetails.tsx`
- Modify: `src/diagram/ModuleDetails.module.css`

**Implementation:**

Add Slate editors for units and documentation, following the inline pattern from `VariableDetails.tsx` (lines 392-418). Import Slate dependencies:

```typescript
import { createEditor, type Descendant } from 'slate';
import { Editable, Slate, withReact } from 'slate-react';
import { withHistory } from 'slate-history';
import { plainSerialize, plainDeserialize } from './drawing/common';
```

Add two Slate editor instances as class fields (same pattern as VariableDetails):
- `unitsEditor` -- single-line, placeholder "Enter units..."
- `docsEditor` -- multi-line, placeholder "Documentation"

On blur from either editor, call `this.props.onUnitsDocsChange(ident, units, docs)`.

Add a delete button at the bottom using `<Button>` from `./components/Button`:
```tsx
<Button color="secondary" onClick={() => this.props.onDelete(variable.ident)}>
  Delete Module
</Button>
```

**No equation editor** -- modules have no equations. No lookup function tab. No simulation chart. This is the key structural difference from VariableDetails.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: add units/docs editors and delete button to ModuleDetails`

<!-- END_TASK_5 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 6-8) -->

<!-- START_TASK_6 -->
### Task 6: Wire ModuleDetails into Editor.getDetails() and add handlers

**Verifies:** module-editing.AC2.1, module-editing.AC2.8, module-editing.AC1.7, module-editing.AC1.8, module-editing.AC1.10, module-editing.AC1.11, module-editing.AC5.3

**Files:**
- Modify: `src/diagram/Editor.tsx` (getDetails at line ~1876, handleEquationChange at line ~1678, new handlers)

**Implementation:**

**Modify getDetails() (line ~1876):**

After getting the `variable` from `model.variables` (line ~1899), check its type. If it's a module, render `ModuleDetails` instead of `VariableDetails`:

```typescript
if (variable.type === 'module') {
  return (
    <div className={styles.varDetails}>
      <ModuleDetails
        key={`md-${this.state.projectVersion}-${this.state.projectOffset}-${ident}`}
        variable={variable}
        viewElement={namedElement}
        project={defined(this.project())}
        currentModelName={this.state.modelName}
        onDelete={this.handleVariableDelete}
        onModelReferenceChange={this.handleModuleModelReferenceChange}
        onUnitsDocsChange={this.handleModuleUnitsDocsChange}
        onDrillIntoModule={this.handleDrillIntoModule}
        onCreateModel={this.handleCreateModelForModule}
        onDuplicateModel={this.handleDuplicateModelForModule}
      />
    </div>
  );
}
```

**Add handleModuleModelReferenceChange handler:**

When user selects a model reference for a module, dispatch an `upsertModule` patch with the new `modelName`:

```typescript
handleModuleModelReferenceChange = async (ident: string, newModelName: string) => {
  const engine = this.engine();
  if (!engine) return;
  const model = this.getModel();
  if (!model) return;
  const variable = model.variables.get(ident);
  if (!variable || variable.type !== 'module') return;

  const op: JsonModelOperation = {
    type: 'upsertModule',
    payload: {
      module: {
        name: variable.ident,
        modelName: newModelName,
        references: variable.references.map(r => ({ src: r.src, dst: r.dst })),
        units: variable.units || undefined,
        documentation: variable.documentation || undefined,
      },
    },
  };

  const patch: JsonProjectPatch = {
    models: [{ name: this.state.modelName, ops: [op] }],
  };

  try {
    await engine.applyPatch(patch, { allowErrors: true });
  } catch (e: unknown) {
    const err = getErrorDetails(e);
    console.error('applyPatch error (model reference update):', err.code, err.message, err.details);
    this.appendModelError(err.message ?? 'Unknown error during model reference update');
    return;
  }

  await this.updateProject(await engine.serializeProtobuf());
};
```

**Add handleModuleUnitsDocsChange handler:**

Similar to above but only updates units and docs:

```typescript
handleModuleUnitsDocsChange = async (ident: string, newUnits: string | undefined, newDocs: string | undefined) => {
  const engine = this.engine();
  if (!engine) return;
  const model = this.getModel();
  if (!model) return;
  const variable = model.variables.get(ident);
  if (!variable || variable.type !== 'module') return;

  const op: JsonModelOperation = {
    type: 'upsertModule',
    payload: {
      module: {
        name: variable.ident,
        modelName: variable.modelName,
        references: variable.references.map(r => ({ src: r.src, dst: r.dst })),
        units: newUnits ?? variable.units ?? undefined,
        documentation: newDocs ?? variable.documentation ?? undefined,
      },
    },
  };

  const patch: JsonProjectPatch = {
    models: [{ name: this.state.modelName, ops: [op] }],
  };

  try {
    await engine.applyPatch(patch, { allowErrors: true });
  } catch (e: unknown) {
    const err = getErrorDetails(e);
    console.error('applyPatch error (module units/docs update):', err.code, err.message, err.details);
    this.appendModelError(err.message ?? 'Unknown error during module update');
    return;
  }

  await this.updateProject(await engine.serializeProtobuf());
};
```

**Add handleCreateModelForModule handler:**

Creates a new empty model via `AddModel` project operation, then sets it as the module's reference:

```typescript
handleCreateModelForModule = async (moduleIdent: string) => {
  const engine = this.engine();
  if (!engine) return;
  // Derive model name from module ident
  const newModelName = moduleIdent;

  // The engine's apply_patch processes projectOps BEFORE model ops
  // (see src/simlin-engine/src/patch.rs line 91: "Apply project-level operations first").
  // This guarantees AddModel creates the model before upsertModule references it.
  const patch: JsonProjectPatch = {
    projectOps: [{ type: 'addModel', payload: { name: newModelName } }],
    models: [{ name: this.state.modelName, ops: [{
      type: 'upsertModule',
      payload: { module: { name: moduleIdent, modelName: newModelName } },
    }]}],
  };

  try {
    await engine.applyPatch(patch, { allowErrors: true });
  } catch (e: unknown) {
    const err = getErrorDetails(e);
    console.error('applyPatch error (create model for module):', err.code, err.message, err.details);
    this.appendModelError(err.message ?? 'Unknown error during model creation');
    return;
  }

  await this.updateProject(await engine.serializeProtobuf());
};
```

**Add handleDuplicateModelForModule handler (AC1.11):**

Duplicates the currently referenced model and sets the copy as the module's new reference. Reads the source model's variables and view, creates a new model via `AddModel`, copies all variables via their respective upsert operations, copies the view via `upsertView`, then updates the module's `modelName` to point to the new copy.

```typescript
handleDuplicateModelForModule = async (moduleIdent: string, sourceModelName: string) => {
  const engine = this.engine();
  if (!engine) return;
  const project = this.project();
  if (!project) return;

  const sourceModel = project.models.get(sourceModelName);
  if (!sourceModel) return;

  // Generate unique name for the copy
  const newModelName = this.getUniqueDuplicateName(sourceModelName, project);

  // Build ops to copy all variables from source model
  const variableOps: JsonModelOperation[] = [];
  for (const variable of sourceModel.variables.values()) {
    if (variable.type === 'stock') {
      variableOps.push({ type: 'upsertStock', payload: { stock: stockToJson(variable) } });
    } else if (variable.type === 'flow') {
      variableOps.push({ type: 'upsertFlow', payload: { flow: flowToJson(variable) } });
    } else if (variable.type === 'aux') {
      variableOps.push({ type: 'upsertAux', payload: { aux: auxToJson(variable) } });
    } else if (variable.type === 'module') {
      variableOps.push({ type: 'upsertModule', payload: { module: moduleToJson(variable) } });
    }
  }

  // Copy the view
  if (sourceModel.views.length > 0) {
    variableOps.push({
      type: 'upsertView',
      payload: { index: 0, view: stockFlowViewToJson(sourceModel.views[0]) },
    });
  }

  // Combined patch: create model, copy contents, update module reference
  // Engine processes projectOps before model ops (patch.rs line 91)
  const patch: JsonProjectPatch = {
    projectOps: [{ type: 'addModel', payload: { name: newModelName } }],
    models: [
      { name: newModelName, ops: variableOps },
      { name: this.state.modelName, ops: [{
        type: 'upsertModule',
        payload: { module: { name: moduleIdent, modelName: newModelName } },
      }]},
    ],
  };

  try {
    await engine.applyPatch(patch, { allowErrors: true });
  } catch (e: unknown) {
    const err = getErrorDetails(e);
    console.error('applyPatch error (duplicate model):', err.code, err.message, err.details);
    this.appendModelError(err.message ?? 'Unknown error during model duplication');
    return;
  }

  await this.updateProject(await engine.serializeProtobuf());
};
```

Add a helper to generate unique duplicate names:
```typescript
private getUniqueDuplicateName(baseName: string, project: Project): string {
  let name = `${baseName}_copy`;
  let i = 2;
  while (project.models.has(name)) {
    name = `${baseName}_copy_${i}`;
    i++;
  }
  return name;
}
```

Import `stockToJson`, `flowToJson`, `auxToJson`, `moduleToJson` from `@simlin/core/datamodel` and `stockFlowViewToJson` from `./view-conversion`.

**Add module branch to handleEquationChange (line ~1678):**

Add a `variable.type === 'module'` branch before the else catch-all that dispatches `upsertModule` with only units/docs changes (ignoring equation):

```typescript
} else if (variable.type === 'module') {
  op = {
    type: 'upsertModule',
    payload: {
      module: {
        name: variable.ident,
        modelName: variable.modelName,
        references: variable.references.map(r => ({ src: r.src, dst: r.dst })),
        units: newUnits ?? variable.units ?? undefined,
        documentation: newDocs ?? variable.documentation ?? undefined,
      },
    },
  };
} else {
```

Import `ModuleDetails` from `'./ModuleDetails'`.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: wire ModuleDetails into Editor with handlers`

<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Add shared model banner

**Verifies:** module-editing.AC4.1, module-editing.AC4.2, module-editing.AC4.3, module-editing.AC4.4

**Files:**
- Modify: `src/diagram/Editor.tsx` (render method, below search bar)
- Modify: `src/diagram/Editor.module.css`

**Implementation:**

When inside a module (modelStack is non-empty), show a thin info banner below the search bar if the current model is used by multiple modules.

In the `render()` method, after the search bar, add:

```tsx
{this.getSharedModelBanner()}
```

Implement `getSharedModelBanner()`:

```typescript
getSharedModelBanner(): React.ReactNode {
  const { modelStack, modelName } = this.state;
  if (modelStack.length === 0) return undefined;

  const project = this.project();
  if (!project) return undefined;

  // AC4.4: stdlib models show read-only message
  if (isStdlibModel(modelName)) {
    return (
      <div className={styles.sharedModelBanner}>
        Standard library model (read-only)
      </div>
    );
  }

  // AC4.1, AC4.2: count instances
  const count = countModelInstances(project, modelName);

  // AC4.3: single instance shows no banner
  if (count <= 1) return undefined;

  return (
    <div className={styles.sharedModelBanner}>
      This model is used by {count} modules — changes affect all instances
    </div>
  );
}
```

Import `countModelInstances` from `'./module-details-utils'` and `isStdlibModel` from `'./module-navigation'`.

**CSS for the banner:**

```css
.sharedModelBanner {
  position: absolute;
  top: 60px;  /* below search bar (48px + 8px top + 4px gap) */
  right: 8px;
  width: var(--panel-width-sm);
  padding: 4px 12px;
  font-size: 12px;
  color: #666;
  background: #fff3e0;
  border-radius: 4px;
  box-shadow: 0 1px 3px rgba(0,0,0,0.12);
  text-align: center;
}

[data-theme="dark"] .sharedModelBanner {
  background: #3e2723;
  color: #ffcc80;
}
```

Responsive width matching search bar:
```css
@media (min-width: 900px) and (max-width: 1199.95px) {
  .sharedModelBanner { width: var(--panel-width-md); }
}
@media (min-width: 1200px) {
  .sharedModelBanner { width: var(--panel-width-lg); }
}
```

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: add shared model editing awareness banner`

<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Tests for ModuleDetails and integration

**Verifies:** module-editing.AC2.1, module-editing.AC2.2, module-editing.AC2.3, module-editing.AC2.4, module-editing.AC2.9, module-editing.AC2.10, module-editing.AC4.1

**Files:**
- Create: `src/diagram/tests/module-details.test.tsx`

**Testing:**

Test the ModuleDetails component in isolation using `@testing-library/react` with `@jest-environment jsdom`. Mock callback props with `jest.fn()`.

Build minimal test data: a `Module` variable, a `Project` with models, and view elements.

Test cases:

- **module-editing.AC2.1:** Render ModuleDetails with a module variable. Verify it renders (no crash) and does NOT render an equation editor.

- **module-editing.AC2.2:** Render with a module having `modelName: 'hares'`. Verify the model name 'hares' appears in the output.

- **module-editing.AC2.3:** Render with a module having `references: [{src: 'food', dst: 'input_food'}]`. Verify 'food' and 'input_food' appear in the wiring table.

- **module-editing.AC2.4:** Create a project with a model 'hares' having a public variable. Render ModuleDetails referencing 'hares'. Verify the public variable appears in the output ports list.

- **module-editing.AC2.9:** Create a model with zero `canBeModuleInput` variables. Verify "No inputs configured" or equivalent empty state text appears.

- **module-editing.AC2.10:** Create a model with zero `isPublic` variables. Verify "No public outputs" or equivalent empty state text appears.

- **Model selector:** Verify that project models appear in the selector. Verify that the current model name is excluded. Verify stdlib models appear.

- **module-editing.AC4.1 (instance count):** This is primarily tested through the `countModelInstances` utility (Task 2 tests), but an integration test verifying the banner renders with correct count is valuable.

- **module-editing.AC5.3 (depth > 1):** Create a project with nested modules (main has module A referencing model_a, model_a has module B referencing model_b). Render ModuleDetails for module B with `currentModelName: 'model_a'`. Verify: (1) the available models list excludes 'model_a' (the parent, to prevent cycles), (2) the parent model variables shown for src dropdown come from model_a (not main), (3) the panel renders correctly without errors.

- **module-editing.AC1.11 (duplicate model):** Render ModuleDetails with a module referencing a model. Verify the "Duplicate model" action is present. Click it and verify `onDuplicateModel` is called with the correct module ident and source model name.

**Verification:**
Run: `cd src/diagram && npx jest tests/module-details.test.tsx`
Expected: All tests pass

**Commit:** `diagram: add tests for ModuleDetails component`

<!-- END_TASK_8 -->

<!-- END_SUBCOMPONENT_C -->
