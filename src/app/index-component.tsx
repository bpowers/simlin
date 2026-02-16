// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { createRoot } from 'react-dom/client';

import { baseURL } from '@simlin/core/common';
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
  connectedCallback() {
    const mountPoint = document.createElement('div');
    mountPoint.setAttribute('class', 'model-Editor-full');

    this.attachShadow({ mode: 'closed' }).appendChild(mountPoint);

    const base = `${scriptURL.protocol}//${scriptURL.host}`;

    const username = this.getAttribute('username') || '';
    const projectName = this.getAttribute('projectName') || '';
    const root = createRoot(mountPoint);
    root.render(
      <div className="model-Editor-full">
        <link rel="stylesheet" href="https://fonts.googleapis.com/css?family=Roboto:300,400,500" />
        <link rel="stylesheet" href="https://fonts.googleapis.com/css?family=Roboto+Mono&display=swap" />
        <HostedWebEditor username={username} projectName={projectName} embedded={true} baseURL={base} />
      </div>,
    );
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

customElements.define('sd-model', SDModel);
