// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import type { GitState, ProjectMeta } from '../api';
import { disambiguatedLabels } from '../utils/disambiguate';
import { NewProjectButton } from './NewProjectButton';

type ProjectListProps = Readonly<{
  projects: ReadonlyArray<ProjectMeta>;
  selectedPath: string | null;
  onSelect: (path: string) => void;
  // Invoked when the always-visible "+ New model" affordance creates a
  // file. The parent updates selectedPath; the WS `projectChanged` event
  // refreshes the list naturally so we don't need a refetch hook here.
  onCreated: (path: string) => void;
}>;

export class ProjectList extends React.Component<ProjectListProps> {
  private handleClick = (path: string) => () => {
    this.props.onSelect(path);
  };

  render(): React.ReactNode {
    const { projects, selectedPath, onCreated } = this.props;
    const labelled = disambiguatedLabels(projects);
    return (
      <div className="serve-project-list-wrapper">
        <NewProjectButton onCreated={onCreated} />
        <ul className="serve-project-list" role="list">
          {labelled.map(({ item: project, label }) => {
            const isSelected = project.path === selectedPath;
            return (
              <li
                key={project.path}
                className={'serve-project-list-item' + (isSelected ? ' serve-project-list-item--selected' : '')}
                aria-current={isSelected ? 'true' : undefined}
                onClick={this.handleClick(project.path)}
              >
                <ProjectLabel label={label} />
                <GitChip git={project.git} />
              </li>
            );
          })}
        </ul>
      </div>
    );
  }
}

type ProjectLabelProps = Readonly<{ label: string }>;

// Splits the on-screen label so the directory portion can fade visually
// (CSS `opacity: 0.65` via `.serve-project-list-path-dir`). When the
// label is a bare basename the label renders as a single span — the dir
// span is omitted rather than emitted empty so screen readers don't
// announce a phantom prefix.
function ProjectLabel({ label }: ProjectLabelProps): React.ReactElement {
  const slash = label.lastIndexOf('/');
  if (slash === -1) {
    return <span className="serve-project-list-path">{label}</span>;
  }
  const dir = label.slice(0, slash + 1);
  const base = label.slice(slash + 1);
  return (
    <span className="serve-project-list-path">
      <span className="serve-project-list-path-dir">{dir}</span>
      {base}
    </span>
  );
}

type GitChipProps = Readonly<{
  git: GitState;
}>;

function GitChip({ git }: GitChipProps): React.ReactElement {
  switch (git.kind) {
    case 'tracked':
      return git.dirty ? (
        <span
          className="serve-git-chip serve-git-chip--dirty"
          aria-label="modified"
          title="modified — uncommitted changes"
        >
          modified
        </span>
      ) : (
        <span
          className="serve-git-chip serve-git-chip--clean"
          aria-label="version controlled"
          title="version controlled — clean"
        >
          tracked
        </span>
      );
    case 'untracked':
      return (
        <span
          className="serve-git-chip serve-git-chip--untracked"
          aria-label="not under version control"
          title="not under version control"
        >
          untracked
        </span>
      );
    case 'unavailable':
      return (
        <span
          className="serve-git-chip serve-git-chip--unavailable"
          aria-label="git status unavailable"
          title="git status unavailable"
        >
          --
        </span>
      );
    default: {
      // Compile-time exhaustiveness guard: if a new GitState variant is added,
      // TypeScript will reject the assignment to `never` here.
      const _exhaustive: never = git;
      void _exhaustive;
      return <></>;
    }
  }
}
