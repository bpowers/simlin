// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { createRoot, type Root } from 'react-dom/client';

import { baseURL } from '@simlin/core/common';
import '@simlin/diagram/reset.css';
import 'katex/dist/katex.min.css';
import '@simlin/diagram/theme.css';
import { HostedWebEditor } from '@simlin/diagram/HostedWebEditor';

// try to get the base URL from the src attribute of the current script
// (so that e.g. localhost:3000 works for testing), but fall back to baseURL
// from common if that doesn't work.
const currentScriptSrc =
  document.currentScript && document.currentScript instanceof HTMLScriptElement
    ? document.currentScript.src
    : `${baseURL}/static/js/sd-component.js`;
const scriptURL = new URL(currentScriptSrc);

class SDModel extends HTMLElement {
  // With { mode: 'closed' } the platform never exposes this.shadowRoot, and a
  // shadow root can't be detached or re-attached anyway, so keep our own
  // reference for the life of the element.
  private shadow: ShadowRoot | undefined;
  private reactRoot: Root | undefined;

  // The HTML parser and setAttribute ASCII-lowercase attribute names, so the
  // observed list must use the lowercase spellings ('projectName' would never
  // match and its changes would be silently ignored).
  static readonly observedAttributes = ['username', 'projectname'];

  connectedCallback() {
    // Hosts (especially SPAs) move elements around, and moving a connected
    // node fires disconnectedCallback then connectedCallback synchronously
    // within the one DOM operation. attachShadow throws NotSupportedError if
    // the host already has a shadow root, so attach exactly once and reuse it
    // across reconnects.
    if (this.shadow === undefined) {
      this.shadow = this.attachShadow({ mode: 'closed' });
    }

    this.mountEditor(this.shadow);
  }

  disconnectedCallback() {
    this.unmountEditor();
  }

  attributeChangedCallback(_name: string, oldValue: string | null, newValue: string | null) {
    // Fires per spec even when the value did not change; remounting then
    // would tear down the editor and refetch the identical project for a
    // no-op write.
    if (oldValue === newValue) {
      return;
    }
    // A live username/projectname change is a project SWAP, so it must
    // REMOUNT rather than re-render: HostedWebEditor loads its project once
    // per mount, so a bare re-render would leave the old project's data on
    // screen while save/delete quietly retarget the new identity. This also
    // fires for initial attribute values before connectedCallback and for
    // changes made while detached; the reactRoot guard skips those (a
    // (re)connect reads the current values itself).
    if (this.reactRoot === undefined || this.shadow === undefined) {
      return;
    }
    this.unmountEditor();
    this.mountEditor(this.shadow);
  }

  // A React root can't render again once unmounted, so every mount starts
  // over: drop whatever the previous mount left in the shadow root and build
  // a fresh mount div + fresh React root, rendering from the element's
  // current attribute values.
  private mountEditor(shadow: ShadowRoot) {
    // Defensive: call sites already unmount before remounting, but a missed
    // ordering here would leak a live React tree behind replaceChildren().
    this.unmountEditor();
    shadow.replaceChildren();
    const mountPoint = document.createElement('div');
    mountPoint.setAttribute('class', 'model-Editor-full');
    shadow.appendChild(mountPoint);

    const base = `${scriptURL.protocol}//${scriptURL.host}`;
    const stylesheet = `${base}/static/css/sd-component.css`;

    const username = this.getAttribute('username') || '';
    const projectName = this.getAttribute('projectName') || '';
    this.reactRoot = createRoot(mountPoint);
    this.reactRoot.render(
      <div className="model-Editor-full">
        <link rel="stylesheet" href="https://fonts.googleapis.com/css?family=Roboto:300,400,500" />
        <link rel="stylesheet" href="https://fonts.googleapis.com/css?family=Roboto+Mono&display=swap" />
        <link rel="stylesheet" href={stylesheet} />
        <HostedWebEditor username={username} projectName={projectName} embedded={true} baseURL={base} />
      </div>,
    );
  }

  private unmountEditor() {
    // Unmount so React runs effect cleanups -- that's where the Editor
    // disposes its ProjectController and the engine resources behind it.
    // Without this, removing the element leaks all of that until page unload.
    if (this.reactRoot !== undefined) {
      this.reactRoot.unmount();
      this.reactRoot = undefined;
    }
  }
}

const cssTagId = 'sd-model-style';

// ensure we have reasonable default styles for sd-model tags, but ensure
// we only add the style tag once.
if (!document.getElementById(cssTagId)) {
  const css = `sd-model { display: inline-block; width: 100%; }`;
  const style = document.createElement('style');
  style.id = cssTagId;
  style.type = 'text/css';
  style.appendChild(document.createTextNode(css));
  document.head.appendChild(style);
}

// SPA hosts can end up evaluating this script more than once (e.g. a route
// remount re-injecting the script tag), and a second define() for an
// already-registered name throws NotSupportedError. First registration wins --
// including its base URL: the registered class closes over the first
// evaluation's module-scope scriptURL, so the stylesheet/save origin derived
// from document.currentScript sticks for the lifetime of the page.
if (!customElements.get('sd-model')) {
  customElements.define('sd-model', SDModel);
}
