# Module Editing: Test Requirements

Maps every acceptance criterion from the module editing design plan to either an automated test or a documented human verification procedure. Rationalized against implementation decisions in [phase_01.md](phase_01.md) through [phase_04.md](phase_04.md).

## Conventions

- **Test type abbreviations**: `unit` = pure function test, `integration` = WASM engine roundtrip, `component` = React Testing Library
- **Environment**: `node` for non-DOM tests (pure functions, WASM integration), `jsdom` for React component tests
- **File locations**: All TypeScript tests in `src/diagram/tests/` following existing patterns
- **Pattern preference**: Extract pure functions, test directly (per CLAUDE.md and `docs/dev/typescript.md`)

---

## AC1: Module Creation

| AC | Criterion | Test Type | File | What to Verify |
|---|---|---|---|---|
| AC1.1 | Module tool in SpeedDial | unit | module-creation.test.ts | `selectedTool` union includes `'module'`; handler pattern matches stock/aux |
| AC1.2 | Click-to-place module | unit | module-creation.test.ts | `ModuleViewElement` creation at (x,y) produces correct structure |
| AC1.3 | Inline name editing | **human** | -- | Shared `editNameOnPointerUp` flow; place module, confirm editing begins |
| AC1.4 | No model ref, empty refs | unit + integration | module-creation.test.ts, module-patch.test.ts | `upsertModule` payload has `modelName: ''`, `references: []`; engine roundtrip preserves |
| AC1.5 | Warning indicator | unit | module-creation.test.ts | `anyModuleHasModelReference` returns true -> hasWarning reflects engine error |
| AC1.6 | Warning suppression | unit | module-creation.test.ts | `anyModuleHasModelReference` returns false for all-unconfigured modules |
| AC1.7 | Model ref in details | component | module-details.test.tsx | ModuleDetails shows model selector; selecting triggers `onModelReferenceChange` |
| AC1.8 | Clear warning on ref set | unit + integration | module-creation.test.ts, module-patch.test.ts | Non-empty modelName clears engine error |
| AC1.9 | Stdlib model as ref | unit + component | module-navigation.test.ts, module-details.test.tsx | `isStdlibModel` returns true; stdlib models in selector |
| AC1.10 | Create new model | component + integration | module-details.test.tsx, module-patch.test.ts | "Create new model" calls `onCreateModel`; combined AddModel+upsertModule patch works |
| AC1.11 | Duplicate model | component | module-details.test.tsx | "Duplicate" visible when ref set; calls `onDuplicateModel` with correct args |
| AC1.12 | Cycle exclusion | unit | module-details-utils.test.ts | `wouldCreateCycle` DFS detects cycles; `getAvailableModels` excludes them |
| AC1.13 | Cancel removes module | **human** | -- | Shared `handleEditingNameCancel` flow; press Escape, verify module removed |

## AC2: Module Details Panel

| AC | Criterion | Test Type | File | What to Verify |
|---|---|---|---|---|
| AC2.1 | ModuleDetails shown | component | module-details.test.tsx | Renders without crash; no equation editor |
| AC2.2 | Shows model name | component | module-details.test.tsx | `modelName: 'hares'` appears in output |
| AC2.3 | Input wiring table | component | module-details.test.tsx | `references` src/dst appear in table |
| AC2.4 | Output ports list | unit + component | module-details-utils.test.ts, module-details.test.tsx | `getPublicVariables` filters by `isPublic`; public vars shown in list |
| AC2.5 | Add input reference | unit + component | module-wiring.test.ts, module-wiring-ui.test.tsx | `addReference` appends; rejects duplicates; "Add Input" button works |
| AC2.6 | Remove input reference | unit + component | module-wiring.test.ts, module-wiring-ui.test.tsx | `removeReference` removes at index; remove button works |
| AC2.7 | Units/docs editable | **human** | -- | Slate editors; edit units/docs, blur, verify persistence |
| AC2.8 | Open Model button | component | module-details.test.tsx | Button calls `onDrillIntoModule` with correct args |
| AC2.9 | Empty input section | unit + component | module-details-utils.test.ts, module-details.test.tsx | `getInputPorts` returns empty; "No inputs configured" shown |
| AC2.10 | Empty output section | unit + component | module-details-utils.test.ts, module-details.test.tsx | `getPublicVariables` returns empty; "No public outputs" shown |

## AC3: Hierarchical Model Navigation

| AC | Criterion | Test Type | File | What to Verify |
|---|---|---|---|---|
| AC3.1 | Double-click drill-in | unit | module-navigation.test.ts | `pushModule` appends entry; `currentModelName` returns target |
| AC3.2 | Back arrow | unit + component | module-navigation.test.ts, module-navigation-ui.test.tsx | `popModule` restores state; back arrow renders when nested |
| AC3.3 | Breadcrumb path | unit + component | module-navigation.test.ts, module-navigation-ui.test.tsx | `breadcrumbSegments` returns correct labels; renders correctly |
| AC3.4 | Breadcrumb click | unit + component | module-navigation.test.ts, module-navigation-ui.test.tsx | `navigateToLevel` truncates correctly; click calls handler |
| AC3.5 | Restore selection/scroll | unit | module-navigation.test.ts | Push with specific state, pop, verify exact restoration |
| AC3.6 | Scoped search | **human** | -- | Emergent from architecture; verify only current model vars in search |
| AC3.7 | 3+ nesting levels | unit + component | module-navigation.test.ts, module-navigation-ui.test.tsx | 3-entry stack operations work; all segments render |
| AC3.8 | Stdlib read-only | unit + component | module-navigation.test.ts, module-navigation-ui.test.tsx | `isStdlibModel` correct for all 9; read-only badge renders |

## AC4: Shared Model Editing Awareness

| AC | Criterion | Test Type | File | What to Verify |
|---|---|---|---|---|
| AC4.1 | Shared model banner | unit | module-details-utils.test.ts | `countModelInstances` returns correct count |
| AC4.2 | Correct count update | unit | module-details-utils.test.ts | Pure function recomputes from current project state |
| AC4.3 | Single instance no banner | unit | module-details-utils.test.ts | Count of 1 returns no banner |
| AC4.4 | Stdlib read-only message | unit | module-navigation.test.ts | `isStdlibModel` discriminates; banner shows read-only text |

## AC5: Composable Foundation

| AC | Criterion | Test Type | File | What to Verify |
|---|---|---|---|---|
| AC5.1 | Identical at any depth | unit + component | module-navigation.test.ts, module-navigation-ui.test.tsx | Same operations produce same results at depth 1, 2, 3 |
| AC5.2 | Module creation at any depth | unit | module-creation.test.ts | Patch targets `modelName` (inner model), not `'main'` |
| AC5.3 | ModuleDetails at any depth | component | module-details.test.tsx | At depth > 1: correct parent model vars, correct cycle exclusion |
| AC5.4 | No depth special-casing | unit | module-navigation.test.ts | Behavioral symmetry across depths; no depth-1 vs depth-N branches |

---

## Test File Summary

| File | Phase | Environment | Type | ACs Covered |
|---|---|---|---|---|
| `src/diagram/tests/module-navigation.test.ts` | 1 | node | unit | AC3.1-3.5, AC3.7, AC3.8, AC4.4, AC5.1, AC5.4 |
| `src/diagram/tests/module-navigation-ui.test.tsx` | 1 | jsdom | component | AC3.2-3.4, AC3.7, AC3.8, AC5.1 |
| `src/diagram/tests/module-creation.test.ts` | 2 | node | unit | AC1.1, AC1.2, AC1.4-AC1.6, AC5.2 |
| `src/diagram/tests/module-patch.test.ts` | 2 | node | integration | AC1.4, AC1.8, AC1.10 |
| `src/diagram/tests/module-details-utils.test.ts` | 3 | node | unit | AC1.12, AC2.4, AC2.9, AC2.10, AC4.1-AC4.3 |
| `src/diagram/tests/module-details.test.tsx` | 3 | jsdom | component | AC1.7, AC1.9-AC1.11, AC2.1-AC2.4, AC2.8-AC2.10, AC5.3 |
| `src/diagram/tests/module-wiring.test.ts` | 4 | node | unit | AC2.5, AC2.6 |
| `src/diagram/tests/module-wiring-ui.test.tsx` | 4 | jsdom | component | AC2.5, AC2.6 |

---

## Human Verification Summary

| AC | Criterion | Justification | Verification Approach |
|---|---|---|---|
| AC1.3 | Inline name editing | Shared `editNameOnPointerUp` flow; no module-specific code | Place module, confirm editing begins, type name, press Enter |
| AC1.13 | Cancel removes module | Shared `handleEditingNameCancel` using `inCreationUid` | Select module tool, click canvas, press Escape, verify removal |
| AC2.7 | Units/docs editable | Slate editors follow VariableDetails pattern; fragile in jsdom | Edit units field, blur, re-select, verify persistence |
| AC3.6 | Scoped search | Emergent from `getView()` returning current model's elements | Navigate into child model, open search, verify only child vars shown |
