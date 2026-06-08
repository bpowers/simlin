// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { LineChart, ChartSeries } from './LineChart';
import { createEditor, Descendant, Transforms } from 'slate';
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
import { caretOffsetForClick, caretOffsetWithinSpan, RenderedGlyph } from './equation-caret';
import {
  HighlightRange,
  applyToAllPrefix,
  byteOffsetToUtf16,
  highlightSpansForLines,
  slatePointForOffset,
} from './equation-highlight';
import { LookupEditor } from './LookupEditor';
import { variableDetailsView } from './variable-details-display';
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

interface VariableDetailsState {
  equationContents: Descendant[];
  equationEditor: CustomEditor;
  unitsContents: Descendant[];
  unitsEditor: CustomEditor;
  notesContents: Descendant[];
  notesEditor: CustomEditor;
  editingEquation: boolean;
  latexEquation: string | undefined;
  latexLoading: boolean;
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
// Primary path: find the innermost source-annotated span the click landed in
// (`data-eqnloc` for a syntax node, `data-oploc` for an operator gap) and
// resolve the click within it. Fallback: the rendered LaTeX has no annotations
// (engine produced none; preview shows raw text), so reconstruct from the
// glyph boxes -- or, if KaTeX rendered nothing measurable, a coarse
// proportional mapping over the preview's content box.
function caretOffsetForPreviewClick(
  host: HTMLElement,
  clicked: Element | null,
  clientX: number,
  clientY: number,
  equationStr: string,
): number {
  const annotated = clicked?.closest('[data-eqnloc],[data-oploc]') ?? null;
  if (annotated instanceof HTMLElement && host.contains(annotated)) {
    const isOperatorGap = annotated.dataset.oploc !== undefined;
    const range = parseLocAttr(isOperatorGap ? annotated.dataset.oploc : annotated.dataset.eqnloc);
    if (range) {
      // eqnloc/oploc carry byte offsets into the raw equation; convert to
      // UTF-16 indices in the displayed string (which may be prefixed for
      // apply-to-all equations) before mapping the click.
      const rawStart = rawEquationStart(equationStr, false);
      const raw = equationStr.slice(rawStart);
      const spanStart = rawStart + byteOffsetToUtf16(raw, range[0]);
      const spanEnd = rawStart + byteOffsetToUtf16(raw, range[1]);
      const glyphs = collectRenderedGlyphs(annotated);
      return caretOffsetWithinSpan(glyphs, clientX, clientY, equationStr, spanStart, spanEnd, isOperatorGap);
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

export class VariableDetails extends React.PureComponent<VariableDetailsProps, VariableDetailsState> {
  private _latexRequestId = 0;
  private _mounted = false;

  constructor(props: VariableDetailsProps) {
    super(props);

    const { variable } = props;

    const equation = highlightErrors(
      scalarEquationFor(variable),
      props.variable.errors,
      props.variable.unitErrors,
      false,
    );
    const units = highlightErrors(props.variable.units, props.variable.errors, props.variable.unitErrors, true);

    this.state = {
      equationEditor: withHistory(withReact(createEditor())) as unknown as CustomEditor,
      equationContents: equation,
      unitsEditor: withHistory(withReact(createEditor())) as unknown as CustomEditor,
      unitsContents: units,
      notesEditor: withHistory(withReact(createEditor())) as unknown as CustomEditor,
      notesContents: descendantsFromString(props.variable.documentation),
      editingEquation: !!(props.variable.errors && props.variable.errors.length > 0),
      latexEquation: undefined,
      latexLoading: false,
    };
  }

  componentDidMount() {
    this._mounted = true;
    this.loadLatex();
  }

  componentWillUnmount() {
    this._mounted = false;
  }

  componentDidUpdate(prevProps: VariableDetailsProps) {
    if (prevProps.viewElement.ident !== this.props.viewElement.ident) {
      this.loadLatex();
    }
  }

  private async loadLatex() {
    const { getLatexEquation, viewElement } = this.props;
    if (!getLatexEquation) return;

    const ident = viewElement.ident;
    if (!ident) return;

    const requestId = ++this._latexRequestId;
    this.setState({ latexLoading: true, latexEquation: undefined });
    try {
      const latex = await getLatexEquation(ident);
      if (requestId !== this._latexRequestId || !this._mounted) return;
      this.setState({ latexEquation: latex, latexLoading: false });
    } catch {
      if (requestId !== this._latexRequestId || !this._mounted) return;
      this.setState({ latexEquation: undefined, latexLoading: false });
    }
  }

  handleEquationChange = (equation: Descendant[]): void => {
    this.setState({ equationContents: equation });
  };

  handleVariableDelete = (): void => {
    this.props.onDelete(defined(this.props.viewElement.ident));
  };

  handleUnitsChange = (equation: Descendant[]): void => {
    this.setState({ unitsContents: equation });
  };

  handleNotesChange = (equation: Descendant[]): void => {
    this.setState({ notesContents: equation });
  };

  handleEquationCancel = (): void => {
    this.setState({
      equationContents: descendantsFromString(scalarEquationFor(this.props.variable)),
      unitsContents: descendantsFromString(this.props.variable.units),
      notesContents: descendantsFromString(this.props.variable.documentation),
      editingEquation: false,
    });
  };

  handleEquationSave = (): void => {
    const { equationContents, unitsContents, notesContents } = this.state;
    const initialEquation = scalarEquationFor(this.props.variable);
    const initialUnits = this.props.variable.units;
    const initialDocs = this.props.variable.documentation;

    const newEquation = stringFromDescendants(equationContents);
    const newUnits = stringFromDescendants(unitsContents);
    const newDocs = stringFromDescendants(notesContents);
    const equation = initialEquation !== newEquation ? newEquation : undefined;
    const units = initialUnits !== newUnits ? newUnits : undefined;
    const docs = initialDocs !== newDocs ? newDocs : undefined;
    if (equation !== undefined || units !== undefined || docs != undefined) {
      this.props.onEquationChange(defined(this.props.viewElement.ident), equation, units, docs);
    }
  };

  formatValue = (value: number): string => {
    return value.toFixed(3);
  };

  handleTabChange = (event: React.SyntheticEvent, newValue: number) => {
    this.props.onActiveTabChange(newValue);
  };

  handleAddLookupTable = (): void => {
    const ident = defined(this.props.viewElement.ident);
    const gf: GraphicalFunction = {
      kind: 'continuous',
      xScale: { min: 0, max: 1 },
      yScale: { min: 0, max: 1 },
      xPoints: undefined,
      yPoints: [0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
    };
    this.props.onTableChange(ident, gf);
  };

  renderLeaf = (props: RenderLeafProps) => {
    const leaf = props.leaf as FormattedText;
    const isError = !!leaf.error;
    const isWarning = !!leaf.warning;
    const className = isError ? styles.eqnError : isWarning ? styles.eqnWarning : undefined;
    return (
      <span {...props.attributes} className={className}>
        {props.children}
      </span>
    );
  };

  renderEquation() {
    const { equationContents } = this.state;
    const initialEquation = scalarEquationFor(this.props.variable);
    const initialUnits = this.props.variable.units;
    const initialDocs = this.props.variable.documentation;

    const data: Readonly<Array<Series>> | undefined = this.props.variable.data;

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
      initialUnits !== stringFromDescendants(this.state.unitsContents) ||
      initialDocs !== stringFromDescendants(this.state.notesContents);

    const detailsView = variableDetailsView(this.props.variable);
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

    let chartOrErrors;
    if (!detailsView.showChart) {
      // Equation/compile errors mean the variable produced no valid data, so
      // the error list replaces the chart.
      const errorList = detailsView.equationErrors.map((error, i) => (
        <div key={`eqn-${i}`} className={styles.errorList}>
          error: {errorCodeDescription(error.code)}
        </div>
      ));
      chartOrErrors = [...errorList, ...unitWarnings];
    } else {
      chartOrErrors = (
        <>
          <LineChart height={300} series={chartSeries} yDomain={[yMin, yMax]} tooltipFormatter={this.formatValue} />
          {unitWarnings}
        </>
      );
    }

    // Only genuine equation/compile errors force the raw editor open (so the
    // highlight is visible); non-fatal unit warnings keep the preview, the
    // same way they keep the chart (see variableDetailsView).
    const showPreview = detailsView.equationErrors.length === 0 && !this.state.editingEquation;

    const equationStr = stringFromDescendants(equationContents);
    let latexHTML: string | undefined;
    if (showPreview && this.state.latexEquation !== undefined) {
      try {
        // `displayMode` so it renders block-style; `trust` (scoped to
        // \htmlData) so the engine's source-range annotations survive.
        latexHTML = katex.renderToString(this.state.latexEquation, {
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
          <div className={styles.eqnPreview} onClick={(e) => this.handlePreviewClick(e, equationStr)}>
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
          <Slate
            editor={this.state.equationEditor}
            initialValue={this.state.equationContents}
            onChange={this.handleEquationChange}
          >
            <Editable
              className={styles.eqnEditor}
              renderLeaf={this.renderLeaf}
              placeholder="Enter an equation..."
              spellCheck={false}
              autoFocus
              onBlur={() => {
                this.handleEquationSave();
                // Stay in editing mode only for genuine equation errors --
                // the same gating as showPreview; unit warnings render under
                // the chart and shouldn't pin the raw editor open.
                if (!this.props.variable.errors || this.props.variable.errors.length === 0) {
                  this.setState({ editingEquation: false });
                }
              }}
              onKeyDown={(e) => {
                if (e.key === 'Escape') {
                  this.setState({ editingEquation: false });
                }
              }}
            />
          </Slate>
        )}

        <Slate
          editor={this.state.unitsEditor}
          initialValue={this.state.unitsContents}
          onChange={this.handleUnitsChange}
        >
          <Editable
            className={styles.unitsEditor}
            renderLeaf={this.renderLeaf}
            placeholder="Enter units..."
            spellCheck={false}
            onBlur={this.handleEquationSave}
          />
        </Slate>

        <Slate
          editor={this.state.notesEditor}
          initialValue={this.state.notesContents}
          onChange={this.handleNotesChange}
        >
          <Editable
            className={styles.notesEditor}
            renderLeaf={this.renderLeaf}
            placeholder="Documentation"
            spellCheck={false}
            onBlur={this.handleEquationSave}
          />
        </Slate>

        <div className={styles.cardActions}>
          <Button size="small" color="secondary" onClick={this.handleVariableDelete} className={styles.buttonLeft}>
            Delete
          </Button>
          <div className={styles.buttonRight}>
            <Button size="small" color="primary" disabled={!equationActionsEnabled} onClick={this.handleEquationCancel}>
              Cancel
            </Button>
            <Button size="small" color="primary" disabled={!equationActionsEnabled} onClick={this.handleEquationSave}>
              Save
            </Button>
          </div>
        </div>

        <hr />
        <br />
        {chartOrErrors}
      </div>
    );
  }

  handlePreviewClick = (e: React.MouseEvent<HTMLDivElement>, equationStr: string): void => {
    const target = e.currentTarget as HTMLElement;
    const clicked = e.target instanceof Element ? e.target : null;
    const offset = caretOffsetForPreviewClick(target, clicked, e.clientX, e.clientY, equationStr);

    this.setState({ editingEquation: true }, () => {
      // Focus and place the caret once the editable equation editor has rendered.
      requestAnimationFrame(() => {
        try {
          const editor = this.state.equationEditor;
          ReactEditor.focus(editor);
          // The Slate document is one element per line; convert the flat
          // offset to a (line, column) point so multi-line equations place
          // the caret on the right line.
          const point = slatePointForOffset(equationStr, offset);
          Transforms.select(editor, {
            anchor: { path: [...point.path], offset: point.offset },
            focus: { path: [...point.path], offset: point.offset },
          });
        } catch {
          // ignore if selection fails; the user can click to place the caret
        }
      });
    });
  };

  handleLookupChange = (ident: string, newTable: GraphicalFunction | null) => {
    this.props.onTableChange(ident, newTable);
  };

  renderLookup() {
    const { variable } = this.props;

    let table;
    if (variableGf(variable)) {
      table = <LookupEditor variable={variable} onLookupChange={this.handleLookupChange} />;
    } else {
      table = (
        <div className={styles.cardContent}>
          <Button
            variant="contained"
            color="secondary"
            onClick={this.handleAddLookupTable}
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
  }

  render() {
    const { activeTab, viewElement } = this.props;

    const equationType = viewElement.type === 'stock' ? 'Initial Value' : 'Equation';
    const content = activeTab === 0 ? this.renderEquation() : this.renderLookup();
    const lookupTab = viewElement.type === 'stock' ? undefined : <Tab label="Lookup Function" />;

    return (
      <div className={styles.card}>
        <Tabs
          className={styles.inner}
          variant="fullWidth"
          value={activeTab}
          indicatorColor="primary"
          textColor="primary"
          onChange={this.handleTabChange}
          aria-label="Equation details selector"
        >
          <Tab label={equationType} />
          {lookupTab}
        </Tabs>

        {content}
      </div>
    );
  }
}
