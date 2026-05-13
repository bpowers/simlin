// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { LineChart, ChartSeries } from './LineChart';
import { createEditor, Descendant, Text, Transforms } from 'slate';
import { withHistory } from 'slate-history';
import { Editable, ReactEditor, RenderLeafProps, Slate, withReact } from 'slate-react';
import Button from './components/Button';
import { Tabs, Tab } from './components/Tabs';
import katex from 'katex';
import { Dark2 } from './colors';

import {
  ViewElement,
  Variable,
  GraphicalFunction,
  EquationError,
  UnitError,
  variableGf,
} from '@simlin/core/datamodel';

import { defined, Series } from '@simlin/core/common';
import { at } from '@simlin/core/collections';
import { plainDeserialize, plainSerialize } from './drawing/common';
import { CustomElement, FormattedText, CustomEditor } from './drawing/SlateEditor';
import { caretOffsetForClick, caretOffsetWithinSpan, RenderedGlyph } from './equation-caret';
import { LookupEditor } from './LookupEditor';
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
    return '{apply-to-all:}\n' + variable.equation.equation;
  } else {
    return "{ TODO: arrayed variable editing isn't supported yet}";
  }
}

function highlightErrors(
  s: string,
  errors: readonly EquationError[] | undefined,
  unitErrors: readonly UnitError[] | undefined,
  isUnits: boolean,
): CustomElement[] {
  const result = descendantsFromString(s);
  if (!isUnits && errors && errors.length > 0) {
    const err = at(errors, 0);
    console.log(err);
    if (err.end > 0) {
      const children = defined(result[0]).children as Array<Text>;
      const textChild: string = defined(children[0]).text;

      const beforeText = textChild.substring(0, err.start);
      const errText = textChild.substring(err.start, err.end);
      const afterText = textChild.substring(err.end);

      defined(result[0]).children = [{ text: beforeText }, { text: errText, error: true }, { text: afterText }];
    }
  } else if (unitErrors && unitErrors.length > 0) {
    for (const err of unitErrors) {
      if (isUnits === err.isConsistencyError) {
        continue;
      }
      const children = defined(result[0]).children as Array<Text>;
      const textChild: string = defined(children[0]).text;
      const end = err.end === 0 ? textChild.length : err.end;

      const beforeText = textChild.substring(0, err.start);
      const errText = textChild.substring(err.start, end);
      const afterText = textChild.substring(end);

      const highlighted: FormattedText = isUnits ? { text: errText, error: true } : { text: errText, warning: true };
      defined(result[0]).children = [{ text: beforeText }, highlighted, { text: afterText }];

      break;
    }
  }

  return result;
}

// LaTeX provided by engine (Ast::to_latex, with \htmlData{eqnloc=…} source
// annotations). When the engine couldn't produce LaTeX, the preview falls back
// to rendering the raw equation text (a trivial passthrough, no annotations).
const passthroughLatex = (s: string) => s;

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
      const glyphs = collectRenderedGlyphs(annotated);
      return caretOffsetWithinSpan(glyphs, clientX, clientY, equationStr, range[0], range[1], isOperatorGap);
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

    let chartOrErrors;
    const errors = this.props.variable.errors;
    const unitErrors = this.props.variable.unitErrors;
    if (errors || unitErrors) {
      const errorList: Array<React.ReactElement> = [];
      if (errors) {
        errors.forEach((error) => {
          errorList.push(<div className={styles.errorList}>error: {errorCodeDescription(error.code)}</div>);
        });
      }
      if (unitErrors) {
        unitErrors.forEach((error) => {
          const details = error.details;
          errorList.push(
            <div className={styles.errorList}>
              unit error: {errorCodeDescription(error.code)}
              {details ? `: ${details}` : undefined}
            </div>,
          );
        });
      }
      chartOrErrors = errorList;
    } else {
      chartOrErrors = (
        <LineChart height={300} series={chartSeries} yDomain={[yMin, yMax]} tooltipFormatter={this.formatValue} />
      );
    }

    const showPreview = !errors && !unitErrors && !this.state.editingEquation;

    const equationStr = stringFromDescendants(equationContents);
    let latexHTML = '';
    if (showPreview) {
      try {
        const latex = this.state.latexEquation ?? passthroughLatex(equationStr);
        // `displayMode` so it renders block-style; `trust` (scoped to
        // \htmlData) so the engine's source-range annotations survive. Long
        // equations wrap via the .eqnPreview CSS (overflow-wrap: anywhere).
        latexHTML = katex.renderToString(latex, { throwOnError: false, displayMode: true, trust: katexTrust });
      } catch {
        // fall back to plain text
        latexHTML = '';
      }
    }

    return (
      <div className={styles.cardContent}>
        {showPreview ? (
          <div
            className={styles.eqnPreview}
            onClick={(e) => this.handlePreviewClick(e, equationStr)}
            dangerouslySetInnerHTML={{ __html: latexHTML }}
          />
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
                if (!this.props.variable.errors && !this.props.variable.unitErrors) {
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
          Transforms.select(editor, {
            anchor: { path: [0, 0], offset },
            focus: { path: [0, 0], offset },
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
