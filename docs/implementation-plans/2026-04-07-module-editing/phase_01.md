# Module Editing Implementation Plan -- Phase 1: Navigation Foundation

**Goal:** Stack-based model navigation enabling drill-in/out of modules with breadcrumb UI

**Architecture:** Add a `modelStack` array to `EditorState` alongside the existing `modelName` field. Pure navigation functions in a new `module-navigation.ts` file compute state transitions. Editor handlers call these functions and update state atomically. Double-click on module elements triggers drill-in; back arrow and breadcrumb clicks trigger drill-out.

**Tech Stack:** React, TypeScript, CSS Modules

**Scope:** 4 phases from original design (phase 1 of 4)

**Codebase verified:** 2026-04-07

**Testing references:**
- Testing patterns: `src/diagram/CLAUDE.md`, `docs/dev/typescript.md`
- Jest config: `src/diagram/jest.config.js` (jsdom environment, tests in `tests/` subdirectory)
- Preferred pattern: extract pure functions, test directly (see `tests/selection-logic.test.ts`, `tests/keyboard-shortcuts.test.ts`)
- React component tests: `@testing-library/react` with `render`, `fireEvent`, `screen`

---

## Acceptance Criteria Coverage

This phase implements and tests:

### module-editing.AC3: Hierarchical Model Navigation
- **module-editing.AC3.1 Success:** Double-clicking a module on the canvas navigates into its model's diagram
- **module-editing.AC3.2 Success:** Back arrow in search bar navigates to parent model
- **module-editing.AC3.3 Success:** Breadcrumb prefix shows full path (e.g., "main / hares / sub_pop")
- **module-editing.AC3.4 Success:** Clicking a breadcrumb segment navigates directly to that level
- **module-editing.AC3.5 Success:** Navigating back restores previous selection and scroll position
- **module-editing.AC3.6 Success:** "Find in Model" searches only the current model's variables
- **module-editing.AC3.7 Success:** Navigation works correctly at 3+ levels of nesting
- **module-editing.AC3.8 Edge:** Navigating into a stdlib module shows the model structure (read-only indicator)

### module-editing.AC5: Composable Foundation
- **module-editing.AC5.1 Success:** All navigation UI (breadcrumb, back button, search scope) works identically at any nesting depth
- **module-editing.AC5.4 Failure:** No code paths special-case depth-1 vs depth-N (structural requirement)

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Create module-navigation.ts with types and pure navigation functions

**Verifies:** module-editing.AC3.1, module-editing.AC3.2, module-editing.AC3.4, module-editing.AC3.5, module-editing.AC3.7, module-editing.AC5.4

**Files:**
- Create: `src/diagram/module-navigation.ts`

**Implementation:**

Create a new file with the `ModuleStackEntry` type and pure functions for navigation state transitions. These functions take immutable state and return new state -- no side effects.

The key data model insight: each stack entry stores the model that was drilled INTO at that level, plus the PARENT's selection and viewport state (to restore when navigating back). An empty stack means the root model (`'main'`) is active.

```typescript
import { type UID, type Rect } from '@simlin/core/datamodel';

export interface ModuleStackEntry {
  readonly modelName: string;
  readonly moduleIdent: string;
  readonly selection: ReadonlySet<UID>;
  readonly viewBox: Rect;
  readonly zoom: number;
}

export interface NavigateResult {
  readonly newStack: readonly ModuleStackEntry[];
  readonly restoredModelName: string;
  readonly restoredSelection: ReadonlySet<UID>;
  readonly restoredViewBox: Rect;
  readonly restoredZoom: number;
}
```

Pure functions to implement:

1. `currentModelName(stack)` -- returns the model name from the top of the stack, or `'main'` if empty.

2. `pushModule(stack, targetModelName, moduleIdent, currentSelection, currentViewBox, currentZoom)` -- creates a new entry and appends it, returning the new stack. This is called during drill-in: snapshot the current (parent) state, push it, and the new top entry's `modelName` becomes the active model.

3. `popModule(stack)` -- removes the last entry and returns a `NavigateResult` with the restored parent state. Throws if stack is empty.

4. `navigateToLevel(stack, targetLevel)` -- truncates the stack to `targetLevel` entries, restoring state from the entry at index `targetLevel`. Level 0 means "go back to main" (empty stack). Called by breadcrumb clicks.

5. `breadcrumbSegments(stack)` -- returns an array of `{ label: string; level: number }` for rendering. Always starts with `{ label: 'main', level: 0 }`, followed by one entry per stack item using `moduleIdent` as the label.

6. `isStdlibModel(modelName)` -- returns true if the model name is one of the 9 stdlib models: `delay1`, `delay3`, `npv`, `smth1`, `smth3`, `systems_conversion`, `systems_leak`, `systems_rate`, `trend`.

The `navigateToLevel` logic:
- `targetLevel` is 0-indexed where 0 = main (root), 1 = first drill-in, etc.
- Current level = `stack.length`
- To navigate to level L: take the entry at index L (which stores level L's parent state), use it for restoration, and slice the stack to `[0, L)`.
- `popModule(stack)` is equivalent to `navigateToLevel(stack, stack.length - 1)`.
- Throws if `targetLevel < 0`, `targetLevel >= stack.length`, or stack is empty.

**Verification:**
Run: `cd src/diagram && npx jest tests/module-navigation.test.ts`
Expected: Tests from Task 2 pass

**Commit:** `diagram: add module navigation types and pure functions`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Tests for module navigation pure functions

**Verifies:** module-editing.AC3.1, module-editing.AC3.2, module-editing.AC3.4, module-editing.AC3.5, module-editing.AC3.7, module-editing.AC5.4

**Files:**
- Create: `src/diagram/tests/module-navigation.test.ts`

**Testing:**

Tests must verify each AC listed above. Follow the project's pure function testing pattern (see `tests/selection-logic.test.ts` for reference). Use `@jest-environment node` docblock since no DOM is needed.

Test cases to cover:

- **currentModelName:**
  - Empty stack returns `'main'`
  - Single-entry stack returns that entry's modelName
  - Multi-entry stack returns last entry's modelName

- **pushModule:**
  - Pushing onto empty stack creates single-entry stack
  - Pushing onto existing stack appends entry
  - Entry captures provided selection, viewBox, zoom correctly
  - Original stack is not mutated (immutability)

- **popModule:**
  - Popping single-entry stack returns empty stack and `'main'` as restoredModelName
  - Popping multi-entry stack returns correct parent model name
  - Restored selection, viewBox, zoom match what was stored in the popped entry
  - Popping empty stack throws

- **navigateToLevel:**
  - module-editing.AC3.4: Level 0 from depth 2 restores main's state, stack becomes empty
  - Level 1 from depth 3 restores level 1's state, stack becomes length 1
  - Navigating to current level (targetLevel === stack.length) throws
  - Negative level throws
  - Out-of-range positive level (targetLevel > stack.length) throws

- **breadcrumbSegments:**
  - module-editing.AC3.3: Empty stack returns just `[{ label: 'main', level: 0 }]`
  - Single-entry stack returns main + one segment
  - module-editing.AC3.7: Three-entry stack returns main + three segments (3+ levels)

- **isStdlibModel:**
  - module-editing.AC3.8: Returns true for each of the 9 stdlib model names
  - Returns false for user model names like `'hares'`, `'main'`

- **module-editing.AC3.5 (selection/viewport restoration):** Push with specific selection set and viewBox, then pop. Verify restored values match exactly. Test with 3-level deep stack: push twice, pop once, verify intermediate state; pop again, verify root state.

- **module-editing.AC5.4 (no depth special-casing):** Test the same operation at depth 1, 2, and 3 and verify identical behavior patterns. For example, pushModule at any depth should produce the same structural result.

**Verification:**
Run: `cd src/diagram && npx jest tests/module-navigation.test.ts`
Expected: All tests pass

**Commit:** `diagram: add tests for module navigation logic`

<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Integrate navigation state into Editor and add navigation handlers

**Verifies:** module-editing.AC3.1, module-editing.AC3.2, module-editing.AC3.5

**Files:**
- Modify: `src/diagram/Editor.tsx`

**Implementation:**

Add `modelStack` to `EditorState` (at line ~175, alongside existing `modelName`):

```typescript
interface EditorState {
  // ... existing fields ...
  modelName: string;
  modelStack: readonly ModuleStackEntry[];  // NEW
  // ... rest of existing fields ...
}
```

Initialize in constructor (at line ~246, after `modelName: 'main'`):

```typescript
modelStack: [],
```

Import the navigation functions from `./module-navigation`.

Add three handler methods to the `Editor` class:

1. **`handleDrillIntoModule`** -- Called when user double-clicks a module on the canvas. Receives the module's `modelName` (the target model) and `moduleIdent`. Steps:
   - Get current view to capture viewBox and zoom
   - Call `pushModule(this.state.modelStack, targetModelName, moduleIdent, this.state.selection, view.viewBox, view.zoom)`
   - Update state: `modelStack` = new stack, `modelName` = targetModelName, `selection` = empty set, `showDetails` = undefined

2. **`handleNavigateBack`** -- Called when user clicks the back arrow. Steps:
   - If stack is empty, return (no-op)
   - Call `popModule(this.state.modelStack)`
   - Get the restored model's view to apply the restored viewBox/zoom
   - Update state: `modelStack` = result.newStack, `modelName` = result.restoredModelName, `selection` = result.restoredSelection, `showDetails` = undefined
   - Call `queueViewUpdate` with the restored viewBox and zoom applied to the target model's current view

3. **`handleNavigateToLevel`** -- Called when user clicks a breadcrumb segment. Receives `targetLevel: number`. Steps:
   - If targetLevel >= stack.length, return (clicking current level is no-op)
   - Call `navigateToLevel(this.state.modelStack, targetLevel)`
   - Same restoration flow as handleNavigateBack

The viewport restoration requires care: when navigating back, the parent model's `StockFlowView` object already exists in `activeProject.models`. The restored `viewBox` and `zoom` from the stack entry need to be applied to that model's view via `queueViewUpdate`. Use the existing pattern from `handleViewBoxChange` (Editor.tsx:1396).

Important: `modelName` must always be kept in sync with the stack. After any navigation operation, set `modelName` to `currentModelName(newStack)`. All existing code that reads `this.state.modelName` (getModel, getView, setView, patch operations, error display) will automatically use the correct model.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests still pass (no behavior change for empty stack)

**Commit:** `diagram: integrate module navigation state into Editor`

<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Add double-click handling from Canvas through Module for drill-in

**Verifies:** module-editing.AC3.1, module-editing.AC3.6

**Files:**
- Modify: `src/diagram/drawing/Canvas.tsx` (CanvasProps interface at line ~157, module() method at line ~503, SVG element at line ~2382)
- Modify: `src/diagram/drawing/Module.tsx` (ModuleProps interface at line ~20, Module class at line ~62)
- Modify: `src/diagram/Editor.tsx` (getCanvas() method at line ~1420)

**Implementation:**

**Module.tsx changes:**

Add `onDoubleClick` prop to `ModuleProps` (at line ~20):

```typescript
export interface ModuleProps {
  // ... existing props ...
  onDoubleClick?: (element: ModuleViewElement) => void;  // NEW
}
```

Add a double-click handler to the `Module` class:

```typescript
handleDoubleClick = (e: React.MouseEvent<SVGElement>): void => {
  e.preventDefault();
  e.stopPropagation();
  if (this.props.onDoubleClick) {
    this.props.onDoubleClick(this.props.element);
  }
};
```

Add `onDoubleClick={this.handleDoubleClick}` to the `<g>` element (line ~121), alongside the existing `onPointerDown`.

**Canvas.tsx changes:**

Add `onDrillIntoModule` prop to `CanvasProps` (at line ~157):

```typescript
export interface CanvasProps {
  // ... existing props ...
  onDrillIntoModule: (moduleIdent: string, targetModelName: string) => void;  // NEW
}
```

Create a handler in the Canvas class that receives a `ModuleViewElement`, looks up the module's `modelName`, and calls the prop:

```typescript
handleModuleDoubleClick = (element: ModuleViewElement): void => {
  const variable = this.props.model.variables.get(element.ident);
  if (variable?.type !== 'module' || !variable.modelName) {
    return;  // module has no model reference yet
  }
  this.props.onDrillIntoModule(element.ident, variable.modelName);
};
```

In the `module()` method (line ~503), pass the new handler:

```typescript
const props: ModuleProps = {
  // ... existing props ...
  onDoubleClick: this.handleModuleDoubleClick,  // NEW
};
```

**Editor.tsx changes:**

In `getCanvas()` (line ~1420), pass the drill-in handler:

```typescript
const onDrillIntoModule = !embedded
  ? this.handleDrillIntoModule
  : (_moduleIdent: string, _targetModelName: string): void => {};

// In the Canvas JSX:
<Canvas
  // ... existing props ...
  onDrillIntoModule={onDrillIntoModule}  // NEW
/>
```

AC3.6 (search scoping) is automatically satisfied: the `Autocomplete` in `getSearchBar()` populates options from `this.getView()?.elements`, which returns the current model's view elements. When `modelName` changes, `getView()` returns the new model's view, so search naturally scopes to the current model.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests still pass

**Commit:** `diagram: add double-click drill-in from Canvas through Module`

<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-7) -->

<!-- START_TASK_5 -->
### Task 5: Modify search bar for breadcrumb prefix and back arrow

**Verifies:** module-editing.AC3.2, module-editing.AC3.3, module-editing.AC3.4, module-editing.AC3.8, module-editing.AC5.1

**Files:**
- Modify: `src/diagram/Editor.tsx` (getSearchBar() method at line ~1593, imports at top)
- Modify: `src/diagram/Editor.module.css`
- Modify: `src/diagram/components/icons.tsx`

**Implementation:**

**icons.tsx -- add SettingsIcon:**

Add a new `SettingsIcon` SVG component following the same pattern as existing icons (e.g., `MenuIcon` at line 25). Use the Material Design settings gear SVG path. This icon provides model properties drawer access when the hamburger is replaced by the back arrow.

Also add a `ChevronRightIcon` for breadcrumb separators, following the same SVG icon pattern.

**Editor.tsx -- modify getSearchBar():**

Import `ArrowBackIcon` from `'./components/icons'` (already exported at icons.tsx:31) and `breadcrumbSegments`, `isStdlibModel` from `'./module-navigation'`.

The search bar layout changes based on whether `modelStack` is non-empty:

When `modelStack.length === 0` (root level -- existing behavior):
- Hamburger icon opens drawer (unchanged)
- Search box (unchanged)
- Divider + Status (unchanged)

When `modelStack.length > 0` (inside a module):
- **Back arrow**: `<ArrowBackIcon />` replaces `<MenuIcon />`, onClick calls `this.handleNavigateBack`
- **Settings icon**: `<SettingsIcon />` added after back arrow, onClick calls `this.handleShowDrawer` (preserves drawer access)
- **Breadcrumb prefix**: Clickable segments before the search box showing the navigation path. Each segment is a `<span>` or `<button>` with the module ident, separated by `<ChevronRightIcon />`. Clicking a segment calls `this.handleNavigateToLevel(level)`.
- **Search box**: Same `<Autocomplete>` but with reduced width (breadcrumb takes space)
- **Divider + Status**: Unchanged

The variables `name`, `placeholder`, `autocompleteOptions`, and `status` are computed by the existing code earlier in `getSearchBar()` (lines 1600-1616) and remain unchanged. The modified section replaces only the JSX return (lines 1618-1642):

```tsx
const { modelStack } = this.state;
const isNested = modelStack.length > 0;
const segments = breadcrumbSegments(modelStack);

return (
  <div className={styles.searchBar}>
    {isNested ? (
      <>
        <IconButton className={styles.menuButton} aria-label="Back" onClick={this.handleNavigateBack} size="small">
          <ArrowBackIcon />
        </IconButton>
        <IconButton className={styles.menuButton} aria-label="Model Properties" onClick={this.handleShowDrawer} size="small">
          <SettingsIcon />
        </IconButton>
        <div className={styles.breadcrumb}>
          {segments.map((seg, i) => {
            const isLast = i === segments.length - 1;
            const isCurrent = seg.level === modelStack.length;
            return (
              <React.Fragment key={seg.level}>
                {i > 0 && <ChevronRightIcon className={styles.breadcrumbSeparator} />}
                {isCurrent ? (
                  <span className={styles.breadcrumbCurrent}>{seg.label}</span>
                ) : (
                  <button
                    className={styles.breadcrumbLink}
                    onClick={() => this.handleNavigateToLevel(seg.level)}
                  >
                    {seg.label}
                  </button>
                )}
              </React.Fragment>
            );
          })}
        </div>
      </>
    ) : (
      <IconButton className={styles.menuButton} aria-label="Menu" onClick={this.handleShowDrawer} size="small">
        <MenuIcon />
      </IconButton>
    )}
    <div className={styles.searchBox}>
      <Autocomplete
        key={name}
        value={name}
        onChange={this.handleSearchChange}
        clearOnEscape={true}
        defaultValue={name}
        options={autocompleteOptions}
        renderInput={(params: AutocompleteRenderInputParams) => {
          if (params.InputProps) {
            params.InputProps.disableUnderline = true;
          }
          return <TextField {...params} variant="standard" placeholder={placeholder} fullWidth />;
        }}
      />
    </div>
    <div className={styles.divider} />
    <Status status={status} onClick={this.handleStatusClick} />
  </div>
);
```

**Stdlib read-only indicator (AC3.8):**

When inside a stdlib model (detected via `isStdlibModel(this.state.modelName)`), show a "(read-only)" label after the breadcrumb or as a small badge. This can be a simple `<span className={styles.readOnlyBadge}>read-only</span>` appended to the breadcrumb area.

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass

**Commit:** `diagram: add breadcrumb navigation and back arrow to search bar`

<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Add CSS styles for breadcrumb and navigation UI

**Verifies:** module-editing.AC3.3, module-editing.AC5.1

**Files:**
- Modify: `src/diagram/Editor.module.css`

**Implementation:**

Add CSS classes for the breadcrumb components. Follow the project's design tokens:
- Spacing: 8px grid
- Font: Roboto, Helvetica, Arial, sans-serif
- Border radius: 4px
- Primary color: #1976d2 (for clickable breadcrumb links)

Classes to add:

```css
.breadcrumb {
  display: flex;
  align-items: center;
  gap: 2px;
  flex: 0 0 auto;
  overflow: hidden;
  max-width: 40%;
  font-size: 13px;
  color: #666;
}

.breadcrumbLink {
  background: none;
  border: none;
  padding: 2px 4px;
  cursor: pointer;
  color: #1976d2;
  font-size: 13px;
  font-family: inherit;
  border-radius: 2px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.breadcrumbLink:hover {
  background: rgba(25, 118, 210, 0.08);
}

.breadcrumbCurrent {
  padding: 2px 4px;
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.breadcrumbSeparator {
  flex: 0 0 auto;
  width: 16px;
  height: 16px;
  color: #999;
}

.readOnlyBadge {
  font-size: 11px;
  color: #999;
  font-style: italic;
  padding: 0 4px;
  white-space: nowrap;
}
```

Also add dark mode support using the existing `[data-theme="dark"]` pattern from `theme.css`:

```css
[data-theme="dark"] .breadcrumbLink {
  color: #90caf9;
}

[data-theme="dark"] .breadcrumbLink:hover {
  background: rgba(144, 202, 249, 0.08);
}

[data-theme="dark"] .breadcrumbCurrent {
  color: #e0e0e0;
}

[data-theme="dark"] .breadcrumbSeparator {
  color: #666;
}
```

**Verification:**
Run: `cd src/diagram && npx jest`
Expected: All existing tests pass (CSS changes don't affect test behavior)

**Commit:** `diagram: add breadcrumb CSS styles with dark mode support`

<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Tests for breadcrumb rendering and navigation UI integration

**Verifies:** module-editing.AC3.2, module-editing.AC3.3, module-editing.AC3.4, module-editing.AC3.7, module-editing.AC3.8, module-editing.AC5.1

**Files:**
- Create: `src/diagram/tests/module-navigation-ui.test.tsx`

**Testing:**

Test the Editor's search bar rendering with different navigation states. Use `@testing-library/react` following the pattern in `tests/variable-details-latex.test.tsx` and `tests/button.test.tsx`.

Since the `Editor` component requires substantial setup (engine, project data), the recommended approach is to extract the breadcrumb rendering into a small pure component or test the breadcrumb logic through the already-tested pure functions in Task 2. If testing the actual rendered output:

- Create a minimal `BreadcrumbBar` component (or inline helper) that takes `modelStack` and callbacks, and test it in isolation
- Or test the `getSearchBar` output by rendering `Editor` with mock project data

Test cases:

- **module-editing.AC3.3 (breadcrumb display):** With a 2-level stack `[{moduleIdent: 'hares', modelName: 'hares'}, {moduleIdent: 'sub_pop', modelName: 'sub_pop'}]`, breadcrumb should render "main", separator, "hares", separator, "sub_pop"

- **module-editing.AC3.2 (back arrow):** When stack is non-empty, verify back arrow icon is rendered (not hamburger menu icon)

- **module-editing.AC3.2 (back arrow at root):** When stack is empty, verify hamburger menu icon is rendered (not back arrow)

- **module-editing.AC3.4 (breadcrumb click):** Click on "main" breadcrumb segment should call handleNavigateToLevel(0)

- **module-editing.AC3.7 (3+ levels):** With a 3-level stack, verify all segments render correctly

- **module-editing.AC3.8 (stdlib indicator):** When modelName is a stdlib model (e.g., 'delay1'), verify read-only badge is shown

- **module-editing.AC5.1 (identical at any depth):** Verify the same structural elements (back arrow, breadcrumb, search box, status) appear at depth 1 and depth 3 with no differences in available UI

**Verification:**
Run: `cd src/diagram && npx jest tests/module-navigation-ui.test.tsx`
Expected: All tests pass

**Commit:** `diagram: add tests for breadcrumb navigation UI`

<!-- END_TASK_7 -->

<!-- END_SUBCOMPONENT_C -->
