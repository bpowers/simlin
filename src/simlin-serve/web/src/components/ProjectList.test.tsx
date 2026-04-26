// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen, fireEvent } from '@testing-library/react';

import { ProjectList } from './ProjectList';
import type { ProjectMeta } from '../api';

function tracked(path: string, dirty: boolean): ProjectMeta {
  return {
    path,
    format: 'stmx',
    mtime: new Date(0).toISOString(),
    size: 0,
    git: { kind: 'tracked', dirty },
    version: 0,
  };
}

function untracked(path: string): ProjectMeta {
  return {
    path,
    format: 'mdl',
    mtime: new Date(0).toISOString(),
    size: 0,
    git: { kind: 'untracked' },
    version: 0,
  };
}

function unavailable(path: string): ProjectMeta {
  return {
    path,
    format: 'xmile',
    mtime: new Date(0).toISOString(),
    size: 0,
    git: { kind: 'unavailable' },
    version: 0,
  };
}

describe('ProjectList', () => {
  test('renders one row per project (AC1.1)', () => {
    const projects = [tracked('a.stmx', false), untracked('b.mdl'), unavailable('c.xmile')];
    render(<ProjectList projects={projects} selectedPath={null} onSelect={() => {}} onCreated={() => {}} />);

    expect(screen.getByText('a.stmx')).not.toBeNull();
    expect(screen.getByText('b.mdl')).not.toBeNull();
    expect(screen.getByText('c.xmile')).not.toBeNull();
  });

  test('renders a tracked-clean badge for AC2.1', () => {
    const projects = [tracked('clean.stmx', false)];
    render(<ProjectList projects={projects} selectedPath={null} onSelect={() => {}} onCreated={() => {}} />);

    expect(screen.getByLabelText(/version controlled/i)).not.toBeNull();
  });

  test('renders a modified badge for AC2.2', () => {
    const projects = [tracked('dirty.stmx', true)];
    render(<ProjectList projects={projects} selectedPath={null} onSelect={() => {}} onCreated={() => {}} />);

    expect(screen.getByLabelText(/modified/i)).not.toBeNull();
  });

  test('renders the not-in-repo warning for AC2.3', () => {
    const projects = [untracked('orphan.mdl')];
    render(<ProjectList projects={projects} selectedPath={null} onSelect={() => {}} onCreated={() => {}} />);

    expect(screen.getByLabelText(/not under version control/i)).not.toBeNull();
  });

  test('renders the git-unavailable indicator', () => {
    const projects = [unavailable('isolated.xmile')];
    render(<ProjectList projects={projects} selectedPath={null} onSelect={() => {}} onCreated={() => {}} />);

    expect(screen.getByLabelText(/git status unavailable/i)).not.toBeNull();
  });

  test('invokes onSelect with the path when a row is clicked', () => {
    const projects = [tracked('a.stmx', false), tracked('b.stmx', true)];
    const calls: Array<string> = [];
    render(<ProjectList projects={projects} selectedPath={null} onSelect={(p) => calls.push(p)} onCreated={() => {}} />);

    fireEvent.click(screen.getByText('b.stmx'));
    expect(calls).toEqual(['b.stmx']);
  });

  test('marks the selected row visibly', () => {
    const projects = [tracked('a.stmx', false), tracked('b.stmx', false)];
    const { container } = render(
      <ProjectList projects={projects} selectedPath="b.stmx" onSelect={() => {}} onCreated={() => {}} />,
    );

    const selected = container.querySelector('[aria-current="true"]');
    expect(selected).not.toBeNull();
    expect(selected?.textContent).toContain('b.stmx');
  });

  test('renders the always-visible NewProjectButton at the top', () => {
    const projects = [tracked('a.stmx', false)];
    render(<ProjectList projects={projects} selectedPath={null} onSelect={() => {}} onCreated={() => {}} />);
    expect(screen.queryByRole('button', { name: /create new model/i })).not.toBeNull();
  });
});
