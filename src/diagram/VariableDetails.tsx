// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { LineChart, ChartSeries } from './LineChart';
import { createEditor, Descendant, Editor, Transforms } from 'slate';
import { withHistory } from 'slate-history';
import { Editable, ReactEditor, RenderLeafProps, Slate, withReact } from 'slate-react';
import Button from './components/Button';
import { Tabs, Tab } from './components/Tabs';
import katex from 'katex';
import { Dark2 } from './colors';

import { ViewElement, Variable, GraphicalFunction, EquationError, UnitError, variableGf } from '@simlin/core/datamodel';

import { defined, Series } from '@simlin/core/common';
import { at } from '@simlin/core/collections';
import { plainDeserialize, plainSerialize } from './drawing/common';
import { CustomElement, FormattedText, CustomEditor } from './drawing/SlateEditor';
import {
  caretOffsetForClick,
  caretOffsetWithinSpan,
  chooseSpanForClick,
  RenderedGlyph,
  SpanBox,
} from './equation-caret';
import {
  HighlightRange,
  applyToAllPrefix,
  byteOffsetToUtf16,
  highlightSpansForLines,
  slatePointForOffset,
} from './equation-highlight';
import { LookupEditor } from './LookupEditor';
import { variableDetailsView } from './variable-details-display';
import { isNewlineChord } from './keyboard-shortcuts';
import { errorCodeDescription } from '@simlin/engine';

import styles from './VariableDetails.module.css';

interface VariableDetailsProps {
  variable: Variable;
  viewElement: ViewElement;
  getLatexEquation?: (ident: string) => Promise<string | undefined>;
  onDelete: (ident: string) => void;
  onEquationChange: (
    ident: string,
    newEquation: string | undefined,
    newUnits: string | undefined,
    newDoc: string | undefined,
  ) => void;
  onTableChange: (ident: string, newTable: GraphicalFunction | null) => void;
  activeTab: number;
  onActiveTabChange: (newActiveTab: number) => void;
}

function stringFromDescendants(children: Descendant[]): string {
  return plainSerialize(children);
}

function descendantsFromString(equation: string): CustomElement[] {
  return plainDeserialize('equation', equation);
}

function scalarEquationFor(variable: Variable): string {
  if (variable.type === 'module') return '';
  if (variable.equation.type === 'scalar') {
    return variable.equation.equation;
  } else if (variable.equation.type === 'applyToAll') {
    return applyToAllPrefix + variable.equation.equation;
  } else {
    return "{ TODO: arrayed variable editing isn't supported yet}";
  }
}

// Engine error offsets are byte offsets into the raw equation; the displayed
// string may carry the apply-to-all prefix in front of it.
function rawEquationStart(displayed: string, isUnits: boolean): number {
  return !isUnits && displayed.startsWith(applyToAllPrefix) ? applyToAllPrefix.length : 0;
}

function highlightErrors(
  s: string,
  errors: readonly EquationError[] | undefined,
  unitErrors: readonly UnitError[] | undefined,
  isUnits: boolean,
): CustomElement[] {
  const rawStart = rawEquationStart(s, isUnits);

  let range: HighlightRange | undefined;
  if (!isUnits && errors && errors.length > 0) {
    const err = at(errors, 0);
    if (err.end > 0) {
      range = { startByte: err.start, endByte: err.end, kind: 'error' };
    }
  } else if (unitErrors && unitErrors.length > 0) {
    for (const err of unitErrors) {
      // Consistency errors point into the equation; definition errors point
      // into the units string. Only apply the range to the field it targets.
      if (isUnits === err.isConsistencyError) {
        continue;
      }
      // end === 0 is the engine's "to the end of the text" convention;
      // byteOffsetToUtf16 clamps, so any large value reads as "the end".
      const endByte = err.end === 0 ? Number.MAX_SAFE_INTEGER : err.end;
      range = { startByte: err.start, endByte, kind: isUnits ? 'error' : 'warning' };
      break;
    }
  }

  return highlightSpansForLines(s, rawStart, range).map((children): CustomElement => ({ type: 'equation', children }));
}

// KaTeX needs `trust` enabled to honor `\htmlData`. Scope it to that one
// command so a user identifier that smuggled in a `\href`/`\url`/etc. is still
// rejected; `\htmlData` only emits inert `data-*` attributes.
const katexTrust = (context: { command: string }): boolean => context.command === '\\htmlData';

// Walk the rendered KaTeX subtree under `root` and return each visible glyph
// with its on-screen box. KaTeX often packs several characters into one span,
// so each character is measured with a one-character Range. Text inside the
// MathML accessibility mirror (`.katex-mathml`, clipped to a 1px box) is
// skipped -- it would yield bogus rects. This is the imperative-shell
// counterpart to the pure caret-mapping logic in equation-caret.ts. `root` is
// either the whole preview <div> (fallback path) or a single
// `\htmlData{eqnloc=…}` span (annotation path).
function collectRenderedGlyphs(root: Element): RenderedGlyph[] {
  const glyphs: RenderedGlyph[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    if (node.parentElement?.closest('.katex-mathml')) {
      continue;
    }
    const text = node.nodeValue ?? '';
    for (let i = 0; i < text.length; i++) {
      const range = document.createRange();
      range.setStart(node, i);
      range.setEnd(node, i + 1);
      const r = range.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) {
        continue;
      }
      glyphs.push({ char: text[i], left: r.left, right: r.right, top: r.top, bottom: r.bottom });
    }
  }
  return glyphs;
}

// Parse a `data-eqnloc`/`data-oploc` attribute value ("START_END", emitted by
// the engine's annotated LaTeX) into a `[start, end)` byte range.
function parseLocAttr(value: string | undefined): readonly [number, number] | undefined {
  if (value === undefined) {
    return undefined;
  }
  const m = /^(\d+)_(\d+)$/.exec(value);
  return m ? [Number(m[1]), Number(m[2])] : undefined;
}

// Map a click on the equation preview to a caret offset in `equationStr`.
// Primary path: gather every source-annotated span under the preview
// (`data-eqnloc` for a syntax node, `data-oploc` for an operator gap) and pick
// the one the click belongs to by geometry (`chooseSpanForClick` -- the
// smallest box containing the point), then resolve within it. Geometry rather
// than DOM ancestry (`Element.closest`) is deliberate: a click on KaTeX layout
// chrome that carries no annotation of its own -- a fraction bar, or the
// `\frac` v-list wrapper that overlays the denominator row -- has only the
// whole composite (`a/b`) as an annotated ancestor, so ancestry-based lookup
// would map the click by a coarse interpolation across the entire equation
// instead of into the operand the pixel sits in. Note the consequence: when
// any annotation exists, every click -- even one in padding outside all
// annotated boxes -- resolves via the nearest annotated span, so the
// glyph-box fallback below only runs for annotation-free renders (the engine
// produced no LaTeX; the preview shows raw text) -- or, if KaTeX rendered
// nothing measurable, a coarse proportional mapping over the preview's
// content box.
function caretOffsetForPreviewClick(host: HTMLElement, clientX: number, clientY: number, equationStr: string): number {
  // eqnloc/oploc carry byte offsets into the raw equation; convert to UTF-16
  // indices in the displayed string (which may be prefixed for apply-to-all
  // equations) before mapping the click.
  const rawStart = rawEquationStart(equationStr, false);
  const raw = equationStr.slice(rawStart);
  const candidates: Array<{
    el: HTMLElement;
    spanStart: number;
    spanEnd: number;
    isOperatorGap: boolean;
    box: SpanBox;
  }> = [];
  for (const el of host.querySelectorAll<HTMLElement>('[data-eqnloc],[data-oploc]')) {
    const isOperatorGap = el.dataset.oploc !== undefined;
    const range = parseLocAttr(isOperatorGap ? el.dataset.oploc : el.dataset.eqnloc);
    if (!range) {
      continue;
    }
    const r = el.getBoundingClientRect();
    candidates.push({
      el,
      spanStart: rawStart + byteOffsetToUtf16(raw, range[0]),
      spanEnd: rawStart + byteOffsetToUtf16(raw, range[1]),
      isOperatorGap,
      box: { left: r.left, right: r.right, top: r.top, bottom: r.bottom },
    });
  }
  if (candidates.length > 0) {
    const idx = chooseSpanForClick(
      candidates.map((c) => c.box),
      clientX,
      clientY,
    );
    if (idx >= 0) {
      const chosen = candidates[idx];
      const glyphs = collectRenderedGlyphs(chosen.el);
      return caretOffsetWithinSpan(
        glyphs,
        clientX,
        clientY,
        equationStr,
        chosen.spanStart,
        chosen.spanEnd,
        chosen.isOperatorGap,
      );
    }
  }
  const glyphs = collectRenderedGlyphs(host);
  if (glyphs.length > 0) {
    return caretOffsetForClick(glyphs, clientX, clientY, equationStr);
  }
  const rect = host.getBoundingClientRect();
  const style = window.getComputedStyle(host);
  const padLeft = parseFloat(style.paddingLeft || '0');
  const padRight = parseFloat(style.paddingRight || '0');
  const usableWidth = Math.max(1, rect.width - padLeft - padRight);
  const clickX = Math.max(0, Math.min(usableWidth, clientX - rect.left - padLeft));
  return Math.max(0, Math.min(equationStr.length, Math.round((clickX / usableWidth) * equationStr.length)));
}

export function VariableDetails(props: VariableDetailsProps): React.ReactElement {
  const { variable, viewElement, getLatexEquation, onDelete, onEquationChange, onTableChange, activeTab } = props;

  // Seed the Slate editors and their contents from props exactly once per mount
  // (lazy useState initializers), mirroring the old constructor. The Editor keys
  // this panel on projectGeneration, so a content change remounts the panel and
  // re-seeds it -- there is deliberately NO prop-sync effect for these fields,
  // which would fight that keyed-remount invariant (see diagram/CLAUDE.md
  // "Details panels are keyed by projectGeneration"). The latex fields below ARE
  // prop-driven (on viewElement.ident) because selecting a different variable
  // without an intervening content edit does not remount the panel.
  const [equationEditor] = React.useState<CustomEditor>(
    () => withHistory(withReact(createEditor())) as unknown as CustomEditor,
  );
  const [equationContents, setEquationContents] = React.useState<Descendant[]>(() =>
    highlightErrors(scalarEquationFor(variable), variable.errors, variable.unitErrors, false),
  );
  const [unitsEditor] = React.useState<CustomEditor>(
    () => withHistory(withReact(createEditor())) as unknown as CustomEditor,
  );
  const [unitsContents, setUnitsContents] = React.useState<Descendant[]>(() =>
    highlightErrors(variable.units, variable.errors, variable.unitErrors, true),
  );
  const [notesEditor] = React.useState<CustomEditor>(
    () => withHistory(withReact(createEditor())) as unknown as CustomEditor,
  );
  const [notesContents, setNotesContents] = React.useState<Descendant[]>(() =>
    descendantsFromString(variable.documentation),
  );
  const [editingEquation, setEditingEquation] = React.useState<boolean>(
    () => !!(variable.errors && variable.errors.length > 0),
  );
  const [latexEquation, setLatexEquation] = React.useState<string | undefined>(undefined);

  // Monotonic request id and mounted flag for the loadLatex race guard, mirroring
  // the class's `_latexRequestId`/`_mounted` instance fields. Refs (not state)
  // because they gate async continuations and must never themselves re-render.
  const latexRequestId = React.useRef(0);
  const mounted = React.useRef(false);

  // Stable references for the load effect so it depends only on viewElement.ident
  // (the class's componentDidMount + componentDidUpdate prev-ident comparison),
  // not on identity changes of the getLatexEquation callback or the viewElement
  // object. A new variable without an intervening content edit does NOT remount
  // this panel, so the latex must reload here rather than via the keyed remount.
  const getLatexEquationRef = React.useRef(getLatexEquation);
  getLatexEquationRef.current = getLatexEquation;
  const ident = viewElement.ident;

  React.useEffect(() => {
    mounted.current = true;

    const loadLatex = async (): Promise<void> => {
      const fn = getLatexEquationRef.current;
      if (!fn) return;
      if (!ident) return;

      const requestId = ++latexRequestId.current;
      // Clear any stale LaTeX up front so the preview falls back to plain text
      // while the new request is in flight (the class also set latexLoading here,
      // but that flag had no rendered effect; the undefined latexEquation is what
      // drives the plain-text fallback).
      setLatexEquation(undefined);
      try {
        const latex = await fn(ident);
        if (requestId !== latexRequestId.current || !mounted.current) return;
        setLatexEquation(latex);
      } catch {
        if (requestId !== latexRequestId.current || !mounted.current) return;
        setLatexEquation(undefined);
      }
    };

    void loadLatex();

    return () => {
      mounted.current = false;
    };
    // Keyed on `ident` only: this mirrors the class's componentDidMount plus the
    // componentDidUpdate prev-ident comparison. getLatexEquation is read through
    // a ref so a new callback identity does not re-fire the request.
  }, [ident]);

  const handleEquationChange = (equation: Descendant[]): void => {
    setEquationContents(equation);
  };

  const handleVariableDelete = (): void => {
    onDelete(defined(viewElement.ident));
  };

  const handleUnitsChange = (equation: Descendant[]): void => {
    setUnitsContents(equation);
  };

  const handleNotesChange = (equation: Descendant[]): void => {
    setNotesContents(equation);
  };

  const handleEquationCancel = (): void => {
    setEquationContents(descendantsFromString(scalarEquationFor(variable)));
    setUnitsContents(descendantsFromString(variable.units));
    setNotesContents(descendantsFromString(variable.documentation));
    setEditingEquation(false);
  };

  const handleEquationSave = (): void => {
    const initialEquation = scalarEquationFor(variable);
    const initialUnits = variable.units;
    const initialDocs = variable.documentation;

    const newEquation = stringFromDescendants(equationContents);
    const newUnits = stringFromDescendants(unitsContents);
    const newDocs = stringFromDescendants(notesContents);
    const equation = initialEquation !== newEquation ? newEquation : undefined;
    const units = initialUnits !== newUnits ? newUnits : undefined;
    const docs = initialDocs !== newDocs ? newDocs : undefined;
    if (equation !== undefined || units !== undefined || docs != undefined) {
      onEquationChange(defined(viewElement.ident), equation, units, docs);
    }
  };

  const formatValue = (value: number): string => {
    return value.toFixed(3);
  };

  const handleTabChange = (_event: React.SyntheticEvent, newValue: number): void => {
    props.onActiveTabChange(newValue);
  };

  const handleAddLookupTable = (): void => {
    const lookupIdent = defined(viewElement.ident);
    const gf: GraphicalFunction = {
      kind: 'continuous',
      xScale: { min: 0, max: 1 },
      yScale: { min: 0, max: 1 },
      xPoints: undefined,
      yPoints: [0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
    };
    onTableChange(lookupIdent, gf);
  };

  const renderLeaf = (leafProps: RenderLeafProps): React.ReactElement => {
    const leaf = leafProps.leaf as FormattedText;
    const isError = !!leaf.error;
    const isWarning = !!leaf.warning;
    const className = isError ? styles.eqnError : isWarning ? styles.eqnWarning : undefined;
    return (
      <span {...leafProps.attributes} className={className}>
        {leafProps.children}
      </span>
    );
  };

  const renderEquation = (): React.ReactElement => {
    const initialEquation = scalarEquationFor(variable);
    const initialUnits = variable.units;
    const initialDocs = variable.documentation;

    const data: Readonly<Array<Series>> | undefined = variable.data;

    let yMin = 0;
    let yMax = 0;
    const chartSeries: ChartSeries[] = [];
    if (data) {
      const colors = Dark2;
      for (let i = 0; i < data.length; i++) {
        const dataset = data[i];
        const name = data.length === 1 ? 'y' : dataset.name;
        const points: Array<{ x: number; y: number }> = [];
        for (let j = 0; j < dataset.time.length; j++) {
          const y = dataset.values[j];
          points.push({ x: dataset.time[j], y });
          if (y < yMin) yMin = y;
          if (y > yMax) yMax = y;
        }
        chartSeries.push({
          name,
          color: colors[i % colors.length],
          points,
        });
      }
    }

    yMin = Math.floor(yMin);
    yMax = Math.ceil(yMax);

    // enable saving and canceling if the equation has changed
    const equationActionsEnabled =
      initialEquation !== stringFromDescendants(equationContents) ||
      initialUnits !== stringFromDescendants(unitsContents) ||
      initialDocs !== stringFromDescendants(notesContents);

    const detailsView = variableDetailsView(variable);
    // Unit errors are non-fatal warnings: the variable still simulates and has
    // data. They are rendered beneath the chart (or alongside equation errors)
    // rather than replacing the results.
    const unitWarnings = detailsView.unitWarnings.map((error, i) => {
      const details = error.details;
      return (
        <div key={`unit-${i}`} className={styles.errorList}>
          unit error: {errorCodeDescription(error.code)}
          {details ? `: ${details}` : undefined}
        </div>
      );
    });

    // Sketch-connector drift is a non-fatal warning: the variable still
    // simulates, so it renders beside the chart like unit warnings rather than
    // replacing the results.
    const connectorWarnings = detailsView.connectorWarnings.map((warning, i) => (
      <div key={`connector-${i}`} className={styles.errorList}>
        {warning.kind === 'missingConnector'
          ? `equation uses ${warning.name} but no connector is drawn from it`
          : `connector from ${warning.name} is not used in the equation`}
      </div>
    ));

    let chartOrErrors;
    if (!detailsView.showChart) {
      // Equation/compile errors mean the variable produced no valid data, so
      // the error list replaces the chart.
      const errorList = detailsView.equationErrors.map((error, i) => (
        <div key={`eqn-${i}`} className={styles.errorList}>
          error: {errorCodeDescription(error.code)}
        </div>
      ));
      chartOrErrors = [...errorList, ...unitWarnings, ...connectorWarnings];
    } else {
      chartOrErrors = (
        <>
          <LineChart height={300} series={chartSeries} yDomain={[yMin, yMax]} tooltipFormatter={formatValue} />
          {unitWarnings}
          {connectorWarnings}
        </>
      );
    }

    // Only genuine equation/compile errors force the raw editor open (so the
    // highlight is visible); non-fatal unit warnings keep the preview, the
    // same way they keep the chart (see variableDetailsView).
    const showPreview = detailsView.equationErrors.length === 0 && !editingEquation;

    const equationStr = stringFromDescendants(equationContents);
    let latexHTML: string | undefined;
    if (showPreview && latexEquation !== undefined) {
      try {
        // `displayMode` so it renders block-style; `trust` (scoped to
        // \htmlData) so the engine's source-range annotations survive.
        latexHTML = katex.renderToString(latexEquation, {
          throwOnError: false,
          displayMode: true,
          trust: katexTrust,
        });
      } catch {
        latexHTML = undefined;
      }
    }

    return (
      <div className={styles.cardContent}>
        {showPreview ? (
          <div className={styles.eqnPreview} onClick={(e) => handlePreviewClick(e, equationStr)}>
            {latexHTML !== undefined ? (
              <span dangerouslySetInnerHTML={{ __html: latexHTML }} />
            ) : (
              // While the engine LaTeX is loading -- or when the engine can't
              // produce LaTeX at all -- show the raw equation as plain text.
              // Feeding raw text to katex.renderToString would mangle it:
              // identifiers like revenue_per_unit render `_` as subscripts.
              <span className={styles.eqnPlain}>{equationStr}</span>
            )}
          </div>
        ) : (
          <Slate editor={equationEditor} initialValue={equationContents} onChange={handleEquationChange}>
            <Editable
              className={styles.eqnEditor}
              renderLeaf={renderLeaf}
              placeholder="Enter an equation..."
              spellCheck={false}
              autoFocus
              onBlur={() => {
                handleEquationSave();
                // Stay in editing mode only for genuine equation errors --
                // the same gating as showPreview; unit warnings render under
                // the chart and shouldn't pin the raw editor open.
                if (!variable.errors || variable.errors.length === 0) {
                  setEditingEquation(false);
                }
              }}
              onKeyDown={(e) => {
                if (e.key === 'Escape') {
                  setEditingEquation(false);
                  return;
                }
                // Cmd/Ctrl+Enter inserts a line break, matching Shift+Enter
                // (which Slate handles for us); see isNewlineChord for why the
                // default key map misses these chords.
                if (isNewlineChord(e)) {
                  e.preventDefault();
                  Editor.insertSoftBreak(equationEditor);
                }
              }}
            />
          </Slate>
        )}

        <Slate editor={unitsEditor} initialValue={unitsContents} onChange={handleUnitsChange}>
          {/* Deliberately no Cmd/Ctrl+Enter newline chord here: units are
              semantically a single line, so we don't want to invite line
              breaks. (Plain Enter does still insert one today -- a
              pre-existing gap in this field, tracked separately.) */}
          <Editable
            className={styles.unitsEditor}
            renderLeaf={renderLeaf}
            placeholder="Enter units..."
            spellCheck={false}
            onBlur={handleEquationSave}
          />
        </Slate>

        <Slate editor={notesEditor} initialValue={notesContents} onChange={handleNotesChange}>
          <Editable
            className={styles.notesEditor}
            renderLeaf={renderLeaf}
            placeholder="Documentation"
            spellCheck={false}
            onBlur={handleEquationSave}
            onKeyDown={(e) => {
              // Documentation is multi-line like the equation; accept the same
              // Cmd/Ctrl+Enter line-break chord Shift+Enter already provides.
              if (isNewlineChord(e)) {
                e.preventDefault();
                Editor.insertSoftBreak(notesEditor);
              }
            }}
          />
        </Slate>

        <div className={styles.cardActions}>
          <Button size="small" color="error" onClick={handleVariableDelete} className={styles.buttonLeft}>
            Delete
          </Button>
          <div className={styles.buttonRight}>
            <Button size="small" color="primary" disabled={!equationActionsEnabled} onClick={handleEquationCancel}>
              Cancel
            </Button>
            <Button size="small" color="primary" disabled={!equationActionsEnabled} onClick={handleEquationSave}>
              Save
            </Button>
          </div>
        </div>

        <div className={styles.chartDivider} />
        {chartOrErrors}
      </div>
    );
  };

  const handlePreviewClick = (e: React.MouseEvent<HTMLDivElement>, equationStr: string): void => {
    const target = e.currentTarget as HTMLElement;
    const offset = caretOffsetForPreviewClick(target, e.clientX, e.clientY, equationStr);

    // Enter editing mode, then focus and place the caret once the editable
    // equation editor has rendered. The class used a setState callback to defer
    // until after commit; requestAnimationFrame always fires after React has
    // committed the synchronous setEditingEquation(true) above, so the editor is
    // mounted by the time we focus it.
    setEditingEquation(true);
    requestAnimationFrame(() => {
      try {
        ReactEditor.focus(equationEditor);
        // The Slate document is one element per line; convert the flat
        // offset to a (line, column) point so multi-line equations place
        // the caret on the right line.
        const point = slatePointForOffset(equationStr, offset);
        Transforms.select(equationEditor, {
          anchor: { path: [...point.path], offset: point.offset },
          focus: { path: [...point.path], offset: point.offset },
        });
      } catch {
        // ignore if selection fails; the user can click to place the caret
      }
    });
  };

  const handleLookupChange = (lookupIdent: string, newTable: GraphicalFunction | null): void => {
    onTableChange(lookupIdent, newTable);
  };

  const renderLookup = (): React.ReactElement => {
    let table;
    if (variableGf(variable)) {
      table = <LookupEditor variable={variable} onLookupChange={handleLookupChange} />;
    } else {
      table = (
        <div className={styles.cardContent}>
          <Button
            variant="contained"
            color="secondary"
            onClick={handleAddLookupTable}
            className={styles.addLookupButton}
          >
            Add lookup table
          </Button>
          <br />
          <i>
            A lookup table is a non-linear function indexed by the variable{"'"}s equation. You edit the function by
            dragging your mouse or finger across the graph.
          </i>
        </div>
      );
    }

    return table;
  };

  const equationType = viewElement.type === 'stock' ? 'Initial Value' : 'Equation';
  const content = activeTab === 0 ? renderEquation() : renderLookup();
  const lookupTab = viewElement.type === 'stock' ? undefined : <Tab label="Lookup Function" />;

  return (
    <div className={styles.card}>
      <Tabs
        className={styles.inner}
        variant="fullWidth"
        value={activeTab}
        indicatorColor="primary"
        textColor="primary"
        onChange={handleTabChange}
        aria-label="Equation details selector"
      >
        <Tab label={equationType} />
        {lookupTab}
      </Tabs>

      {content}
    </div>
  );
}
