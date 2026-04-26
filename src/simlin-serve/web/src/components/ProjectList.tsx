// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import type { GitState, ProjectMeta } from '../api';
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
    return (
      <div className="serve-project-list-wrapper">
        <NewProjectButton onCreated={onCreated} />
        <ul className="serve-project-list" role="list">
          {projects.map((project) => {
            const isSelected = project.path === selectedPath;
            return (
              <li
                key={project.path}
                className={'serve-project-list-item' + (isSelected ? ' serve-project-list-item--selected' : '')}
                aria-current={isSelected ? 'true' : undefined}
                onClick={this.handleClick(project.path)}
              >
                <span className="serve-project-list-path">{project.path}</span>
                <GitChip git={project.git} />
              </li>
            );
          })}
        </ul>
      </div>
    );
  }
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
