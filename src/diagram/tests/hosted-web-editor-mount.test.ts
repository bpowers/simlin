/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// HostedWebEditor's deferred loadProject() -- the GET /api/projects/:user/:name
// that hydrates the editor -- is kicked off in componentDidMount, not the
// constructor. Under React 18 StrictMode (dev) every committed component goes
// componentDidMount -> componentWillUnmount -> componentDidMount on the *same*
// instance without re-running the constructor, and the render phase is
// double-invoked so a second, discarded instance is created. Scheduling the
// load in componentDidMount (and cancelling the timer in componentWillUnmount)
// means: the StrictMode cycle is schedule -> cancel -> schedule, so loadProject
// runs exactly once; and the discarded render-phase instance, which never
// reaches componentDidMount, never fires loadProject onto a zombie `this`
// (which would setState on an instance React never committed). See
// HostedWebEditor.componentDidMount.

import { HostedWebEditor } from '../HostedWebEditor';

type HostedWebEditorInstance = InstanceType<typeof HostedWebEditor>;

function makeEditor(): HostedWebEditorInstance {
  return new HostedWebEditor({
    username: 'alice',
    projectName: 'climate',
    readOnlyMode: false,
  } as HostedWebEditorInstance['props']);
}

describe('HostedWebEditor.componentDidMount() deferred project load', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    jest.useRealTimers();
    jest.restoreAllMocks();
  });

  it('does not schedule loadProject() from the constructor alone', () => {
    // The constructor must be side-effect free: a StrictMode-discarded
    // render-phase instance runs the constructor but never componentDidMount,
    // so a constructor-scheduled timer would call loadProject() -> setState()
    // on an instance React never committed ("Can't call setState on a component
    // that is not yet mounted").
    const editor = makeEditor();
    const loadProjectSpy = jest.spyOn(editor, 'loadProject').mockResolvedValue(undefined);

    jest.runAllTimers();

    expect(loadProjectSpy).not.toHaveBeenCalled();
  });

  it('reschedules loadProject() across a StrictMode mount/unmount/mount cycle', () => {
    const editor = makeEditor();
    const loadProjectSpy = jest.spyOn(editor, 'loadProject').mockResolvedValue(undefined);

    editor.componentDidMount();
    editor.componentWillUnmount();
    editor.componentDidMount();
    expect(loadProjectSpy).not.toHaveBeenCalled();

    jest.runAllTimers();

    expect(loadProjectSpy).toHaveBeenCalledTimes(1);
  });

  it('cancels the pending loadProject() timer on unmount', () => {
    const editor = makeEditor();
    const loadProjectSpy = jest.spyOn(editor, 'loadProject').mockResolvedValue(undefined);

    editor.componentDidMount();
    editor.componentWillUnmount();

    jest.runAllTimers();

    expect(loadProjectSpy).not.toHaveBeenCalled();
  });
});
