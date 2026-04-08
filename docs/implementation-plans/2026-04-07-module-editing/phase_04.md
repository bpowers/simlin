# Module Editing Implementation Plan -- Phase 4: Wiring Editor

**Goal:** Editable input reference wiring in the ModuleDetails panel -- add, remove, and edit module references through dropdown selectors

**Architecture:** Extend the read-only wiring table from Phase 3 with add/remove controls and dropdown selectors for `src` (parent variable) and `dst` (child input port). Reference changes dispatch `upsertModule` patches with the complete updated references array (the engine does full replacement, not merge). Use the existing `Autocomplete` component from `src/diagram/components/Autocomplete.tsx` for constrained selection.

**Tech Stack:** React, TypeScript, CSS Modules

**Scope:** 4 phases from original design (phase 4 of 4)

**Codebase verified:** 2026-04-07

**Testing references:**
- Testing patterns: `src/diagram/CLAUDE.md`, `docs/dev/typescript.md`
- Jest config: `src/diagram/jest.config.js` (jsdom environment, tests in `tests/` subdirectory)
- Preferred pattern: extract pure functions, test directly

**Key finding from codebase investigation:** `upsertModule` in `src/simlin-engine/src/patch.rs` (line 220) does `*existing = variable` -- a complete overwrite. Updating references means sending the full module with the entire new references array. There is no partial/merge update. The existing `Autocomplete` component (`src/diagram/components/Autocomplete.tsx`) provides a downshift-based combobox suitable for variable selection dropdowns.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### module-editing.AC2: Module Details Panel (wiring editing)
- **module-editing.AC2.5 Success:** User can add a new input reference by selecting src (parent variable) and dst (child input port)
- **module-editing.AC2.6 Success:** User can remove an existing input reference

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Create pure functions for reference manipulation

**Verifies:** module-editing.AC2.5, module-editing.AC2.6

**Files:**
- Create: `src/diagram/module-wiring.ts`

**Implementation:**

Create pure functions for manipulating the immutable references array. These are testable independently of the React component.

```typescript
import { type ModuleReference } from '@simlin/core/datamodel';

/**
 * Returns true if a reference with the same src and dst already exists.
 */
export function isDuplicateReference(
  references: readonly ModuleReference[],
  src: string,
  dst: string,
): boolean {
  return references.some(ref => ref.src === src && ref.dst === dst);
}

/**
 * Add a new reference to the array. Returns a new array.
 * Does not add if src and dst are both non-empty and a duplicate already exists.
 */
export function addReference(
  references: readonly ModuleReference[],
  src: string,
  dst: string,
): readonly ModuleReference[] {
  if (src && dst && isDuplicateReference(references, src, dst)) {
    return references;
  }
  return [...references, { src, dst }];
}

/**
 * Remove the reference at the given index. Returns a new array.
 */
export function removeReference(
  references: readonly ModuleReference[],
  index: number,
): readonly ModuleReference[] {
  return references.filter((_, i) => i !== index);
}

/**
 * Update the src of the reference at the given index. Returns a new array.
 */
export function updateReferenceSrc(
  references: readonly ModuleReference[],
  index: number,
  newSrc: string,
): readonly ModuleReference[] {
  return references.map((ref, i) =>
    i === index ? { src: newSrc, dst: ref.dst } : ref
  );
}

/**
 * Update the dst of the reference at the given index. Returns a new array.
 */
export function updateReferenceDst(
  references: readonly ModuleReference[],
  index: number,
  newDst: string,
): readonly ModuleReference[] {
  return references.map((ref, i) =>
    i === index ? { src: ref.src, dst: newDst } : ref
  );
}

/**
 * Get the list of available src variables from the parent model.
 * Returns variable idents that can serve as source wiring: stocks, flows, and auxes.
 * Excludes modules (they can't be wired as inputs).
 */
export function getAvailableSrcVariables(
  parentVariables: ReadonlyMap<string, { type: string; ident: string }>,
): readonly string[] {
  const result: string[] = [];
  for (const v of parentVariables.values()) {
    if (v.type === 'stock' || v.type === 'flow' || v.type === 'aux') {
      result.push(v.ident);
    }
  }
  return result.sort();
}
```

**Verification:**
Run: `cd src/diagram && npx jest tests/module-wiring.test.ts`
Expected: Tests from Task 2 pass

**Commit:** `diagram: add pure functions for module reference manipulation`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Tests for reference manipulation functions

**Verifies:** module-editing.AC2.5, module-editing.AC2.6

**Files:**
- Create: `src/diagram/tests/module-wiring.test.ts`

**Testing:**

Use `@jest-environment node`. Test each pure function:

- **isDuplicateReference:**
  - Returns false for empty references array
  - Returns true when exact src/dst pair exists
  - Returns false when src matches but dst differs (and vice versa)

- **addReference:**
  - Adding to empty array creates single-element array with correct src/dst
  - Adding to existing array appends without modifying original
  - Original array is not mutated (immutability check)
  - Adding duplicate (same non-empty src and dst) returns original array unchanged
  - Adding with empty src or dst allows duplicates (for new row placeholder pattern)

- **removeReference:**
  - Removing from single-element array returns empty array
  - Removing from multi-element array preserves other elements in order
  - Removing at index 0 removes first element
  - Original array is not mutated

- **updateReferenceSrc:**
  - Updates only the target index
  - Other elements are unchanged
  - Original array is not mutated

- **updateReferenceDst:**
  - Same test cases as updateReferenceSrc

- **getAvailableSrcVariables:**
  - Returns stocks, flows, and auxes
  - Excludes modules
  - Returns sorted list
  - Empty variables map returns empty array

**Verification:**
Run: `cd src/diagram && npx jest tests/module-wiring.test.ts`
Expected: All tests pass

**Commit:** `diagram: add tests for module reference manipulation`

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Make wiring table editable in ModuleDetails

**Verifies:** module-editing.AC2.5, module-editing.AC2.6

**Files:**
- Modify: `src/diagram/ModuleDetails.tsx`
- Modify: `src/diagram/ModuleDetails.module.css`

**Implementation:**

Replace the read-only wiring table from Phase 3 with an editable version. The `ModuleDetails` component needs a new prop for reference changes:

```typescript
interface ModuleDetailsProps {
  // ... existing props from Phase 3 ...
  onReferencesChange: (ident: string, newReferences: readonly ModuleReference[]) => void;
}
```

**Wiring table UI changes:**

Each existing reference row gets:
- **src dropdown**: An `Autocomplete` component populated with `getAvailableSrcVariables(parentModel.variables)`. Current value is the reference's `src`. On change, call `updateReferenceSrc` and pass the result to `onReferencesChange`.
- **dst dropdown**: An `Autocomplete` component populated with `getInputPorts(childModel)` (from Phase 3's `module-details-utils.ts`). Current value is the reference's `dst`. On change, call `updateReferenceDst` and pass the result to `onReferencesChange`.
- **Remove button**: An `IconButton` with `<RemoveIcon />` (already exists in `components/icons.tsx`). On click, call `removeReference` and pass the result to `onReferencesChange`.

Below the table, add an "Add Input" button:
- An `IconButton` or `Button` with `<AddIcon />` (already exists in `components/icons.tsx`)
- On click, adds a row with empty src/dst (the user fills in via dropdowns)
- Use `addReference(references, '', '')` and pass the result to `onReferencesChange`
- The new row's dropdowns start empty, prompting the user to select

Import `Autocomplete` from `'./components/Autocomplete'`, `TextField` from `'./components/TextField'`, and `IconButton` from `'./components/IconButton'`.

Each `Autocomplete` renders inside a table cell:

```tsx
<td>
  <Autocomplete
    value={ref.src || null}
    options={availableSrcVars}
    onChange={(_, newValue) => {
      if (newValue) {
        const updated = updateReferenceSrc(variable.references, index, newValue);
        this.props.onReferencesChange(variable.ident, updated);
      }
    }}
    renderInput={(params) => (
      <TextField {...params} variant="standard" placeholder="Select variable" />
    )}
  />
</td>
```

**CSS additions:**

```css
.wiringRow {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-bottom: 4px;
}

.wiringDropdown {
  flex: 1;
  min-width: 0;
}

.addInputButton {
  margin-top: 4px;
}
```

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: make wiring table editable with dropdown selectors`

<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-5) -->

<!-- START_TASK_4 -->
### Task 4: Wire reference changes through Editor to engine

**Verifies:** module-editing.AC2.5, module-editing.AC2.6

**Files:**
- Modify: `src/diagram/Editor.tsx` (add handleModuleReferencesChange handler, update ModuleDetails JSX in getDetails)

**Implementation:**

Add a handler that dispatches `upsertModule` with the complete new references array:

```typescript
handleModuleReferencesChange = async (ident: string, newReferences: readonly ModuleReference[]) => {
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
        references: newReferences.map(r => ({ src: r.src, dst: r.dst })),
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
    console.error('applyPatch error (references update):', err.code, err.message, err.details);
    this.appendModelError(err.message ?? 'Unknown error during references update');
    return;
  }

  await this.updateProject(await engine.serializeProtobuf());
};
```

Update the `ModuleDetails` JSX in `getDetails()` to pass the new prop:

```tsx
<ModuleDetails
  // ... existing props ...
  onReferencesChange={this.handleModuleReferencesChange}
/>
```

The `onReferencesChange` handler follows the same pattern as `handleEquationChange`: build the full `upsertModule` operation, apply the patch, then update the project.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: wire module reference changes through Editor to engine`

<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Tests for wiring editor

**Verifies:** module-editing.AC2.5, module-editing.AC2.6

**Files:**
- Create: `src/diagram/tests/module-wiring-ui.test.tsx`

**Testing:**

Test the editable wiring table in ModuleDetails using `@testing-library/react` with `@jest-environment jsdom`.

Create a test helper that renders `ModuleDetails` with mock props: a module variable with some existing references, a project with a parent model (containing named variables) and a child model (containing input ports).

Test cases:

- **module-editing.AC2.5 (add reference):** Click the "Add Input" button. Verify that `onReferencesChange` is called with a new array one element longer than the original, with the new element having empty src/dst.

- **module-editing.AC2.5 (select src):** Render with one reference row. Interact with the src Autocomplete to select a parent variable. Verify `onReferencesChange` is called with the updated src value.

- **module-editing.AC2.5 (select dst):** Same test for dst Autocomplete with a child model input port.

- **module-editing.AC2.6 (remove reference):** Render with two reference rows. Click the remove button on the first row. Verify `onReferencesChange` is called with an array containing only the second reference.

- **Dropdown options:** Verify the src dropdown contains parent model stocks/flows/auxes but NOT modules. Verify the dst dropdown contains only variables with `canBeModuleInput: true`.

- **Persistence roundtrip:** This is covered by the `upsertModule` engine integration test from Phase 2 Task 8 (the engine replaces the full variable including references).

**Verification:**
Run: `cd src/diagram && npx jest tests/module-wiring-ui.test.tsx`
Expected: All tests pass

**Commit:** `diagram: add tests for wiring editor UI`

<!-- END_TASK_5 -->

<!-- END_SUBCOMPONENT_B -->
