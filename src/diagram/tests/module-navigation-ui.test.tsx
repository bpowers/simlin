// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';
import { BreadcrumbBar } from '../BreadcrumbBar';
import type { ModuleStackEntry } from '../module-navigation';
import type { Rect, UID } from '@simlin/core/datamodel';

function makeRect(x: number, y: number, width: number, height: number): Rect {
  return { x, y, width, height };
}

function makeEntry(modelName: string, moduleIdent: string, selectionItems: ReadonlyArray<UID> = []): ModuleStackEntry {
  return {
    modelName,
    moduleIdent,
    selection: new Set(selectionItems),
    viewBox: makeRect(0, 0, 800, 600),
    zoom: 1,
  };
}

describe('BreadcrumbBar', () => {
  // AC3.2: back arrow at root
  describe('at root level (empty stack)', () => {
    test('renders hamburger menu icon, not back arrow', () => {
      const onBack = jest.fn();
      const onNavigateToLevel = jest.fn();
      const onShowDrawer = jest.fn();

      render(
        <BreadcrumbBar
          modelStack={[]}
          modelName="main"
          onBack={onBack}
          onNavigateToLevel={onNavigateToLevel}
          onShowDrawer={onShowDrawer}
        />,
      );

      // Should have the menu button, not back arrow
      const menuButton = screen.getByLabelText('Menu');
      expect(menuButton).not.toBeNull();
      expect(screen.queryByLabelText('Back')).toBeNull();
    });

    test('clicking menu button calls onShowDrawer', () => {
      const onBack = jest.fn();
      const onNavigateToLevel = jest.fn();
      const onShowDrawer = jest.fn();

      render(
        <BreadcrumbBar
          modelStack={[]}
          modelName="main"
          onBack={onBack}
          onNavigateToLevel={onNavigateToLevel}
          onShowDrawer={onShowDrawer}
        />,
      );

      fireEvent.click(screen.getByLabelText('Menu'));
      expect(onShowDrawer).toHaveBeenCalledTimes(1);
    });

    test('does not render breadcrumb segments', () => {
      const onBack = jest.fn();
      const onNavigateToLevel = jest.fn();
      const onShowDrawer = jest.fn();

      render(
        <BreadcrumbBar
          modelStack={[]}
          modelName="main"
          onBack={onBack}
          onNavigateToLevel={onNavigateToLevel}
          onShowDrawer={onShowDrawer}
        />,
      );

      expect(screen.queryByText('main')).toBeNull();
    });
  });

  // AC3.2: back arrow when nested
  describe('when nested (non-empty stack)', () => {
    test('renders back arrow, not hamburger menu', () => {
      const stack = [makeEntry('hares', 'hares')];
      const onBack = jest.fn();
      const onNavigateToLevel = jest.fn();
      const onShowDrawer = jest.fn();

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="hares"
          onBack={onBack}
          onNavigateToLevel={onNavigateToLevel}
          onShowDrawer={onShowDrawer}
        />,
      );

      const backButton = screen.getByLabelText('Back');
      expect(backButton).not.toBeNull();
      expect(screen.queryByLabelText('Menu')).toBeNull();
    });

    test('renders settings icon for drawer access', () => {
      const stack = [makeEntry('hares', 'hares')];

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="hares"
          onBack={jest.fn()}
          onNavigateToLevel={jest.fn()}
          onShowDrawer={jest.fn()}
        />,
      );

      const settingsButton = screen.getByLabelText('Model Properties');
      expect(settingsButton).not.toBeNull();
    });

    test('clicking back arrow calls onBack', () => {
      const stack = [makeEntry('hares', 'hares')];
      const onBack = jest.fn();

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="hares"
          onBack={onBack}
          onNavigateToLevel={jest.fn()}
          onShowDrawer={jest.fn()}
        />,
      );

      fireEvent.click(screen.getByLabelText('Back'));
      expect(onBack).toHaveBeenCalledTimes(1);
    });

    test('clicking settings icon calls onShowDrawer', () => {
      const stack = [makeEntry('hares', 'hares')];
      const onShowDrawer = jest.fn();

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="hares"
          onBack={jest.fn()}
          onNavigateToLevel={jest.fn()}
          onShowDrawer={onShowDrawer}
        />,
      );

      fireEvent.click(screen.getByLabelText('Model Properties'));
      expect(onShowDrawer).toHaveBeenCalledTimes(1);
    });
  });

  // AC3.3: breadcrumb display
  describe('breadcrumb display', () => {
    test('shows path segments for single-level nesting', () => {
      const stack = [makeEntry('hares', 'hares')];

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="hares"
          onBack={jest.fn()}
          onNavigateToLevel={jest.fn()}
          onShowDrawer={jest.fn()}
        />,
      );

      expect(screen.getByText('main')).not.toBeNull();
      expect(screen.getByText('hares')).not.toBeNull();
    });

    test('shows full path for 2-level nesting', () => {
      const stack = [makeEntry('hares', 'hares'), makeEntry('sub_pop', 'sub_pop')];

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="sub_pop"
          onBack={jest.fn()}
          onNavigateToLevel={jest.fn()}
          onShowDrawer={jest.fn()}
        />,
      );

      expect(screen.getByText('main')).not.toBeNull();
      expect(screen.getByText('hares')).not.toBeNull();
      expect(screen.getByText('sub_pop')).not.toBeNull();
    });

    // AC3.7: 3+ levels
    test('shows full path for 3-level nesting', () => {
      const stack = [
        makeEntry('population', 'population'),
        makeEntry('hares', 'hares'),
        makeEntry('sub_pop', 'sub_pop'),
      ];

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="sub_pop"
          onBack={jest.fn()}
          onNavigateToLevel={jest.fn()}
          onShowDrawer={jest.fn()}
        />,
      );

      expect(screen.getByText('main')).not.toBeNull();
      expect(screen.getByText('population')).not.toBeNull();
      expect(screen.getByText('hares')).not.toBeNull();
      expect(screen.getByText('sub_pop')).not.toBeNull();
    });

    test('current segment is not a clickable button', () => {
      const stack = [makeEntry('hares', 'hares')];

      const { container } = render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="hares"
          onBack={jest.fn()}
          onNavigateToLevel={jest.fn()}
          onShowDrawer={jest.fn()}
        />,
      );

      // "hares" is current level, should be a span not a button
      const breadcrumbArea = container.querySelector('.breadcrumb');
      expect(breadcrumbArea).not.toBeNull();

      const currentSpan = container.querySelector('.breadcrumbCurrent');
      expect(currentSpan).not.toBeNull();
      expect(currentSpan!.textContent).toBe('hares');
      expect(currentSpan!.tagName).toBe('SPAN');
    });

    test('non-current segments are clickable buttons', () => {
      const stack = [makeEntry('hares', 'hares'), makeEntry('sub_pop', 'sub_pop')];

      const { container } = render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="sub_pop"
          onBack={jest.fn()}
          onNavigateToLevel={jest.fn()}
          onShowDrawer={jest.fn()}
        />,
      );

      const links = container.querySelectorAll('.breadcrumbLink');
      // "main" at level 0 and "hares" at level 1 are clickable
      expect(links.length).toBe(2);
      expect(links[0].textContent).toBe('main');
      expect(links[1].textContent).toBe('hares');
    });
  });

  // AC3.4: breadcrumb click navigation
  describe('breadcrumb click navigation', () => {
    test('clicking "main" segment calls onNavigateToLevel(0)', () => {
      const stack = [makeEntry('hares', 'hares'), makeEntry('sub_pop', 'sub_pop')];
      const onNavigateToLevel = jest.fn();

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="sub_pop"
          onBack={jest.fn()}
          onNavigateToLevel={onNavigateToLevel}
          onShowDrawer={jest.fn()}
        />,
      );

      fireEvent.click(screen.getByText('main'));
      expect(onNavigateToLevel).toHaveBeenCalledWith(0);
    });

    test('clicking intermediate segment calls onNavigateToLevel with correct level', () => {
      const stack = [
        makeEntry('population', 'population'),
        makeEntry('hares', 'hares'),
        makeEntry('sub_pop', 'sub_pop'),
      ];
      const onNavigateToLevel = jest.fn();

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="sub_pop"
          onBack={jest.fn()}
          onNavigateToLevel={onNavigateToLevel}
          onShowDrawer={jest.fn()}
        />,
      );

      // "population" is at level 1
      fireEvent.click(screen.getByText('population'));
      expect(onNavigateToLevel).toHaveBeenCalledWith(1);
    });

    test('clicking current segment does not trigger navigation', () => {
      const stack = [makeEntry('hares', 'hares')];
      const onNavigateToLevel = jest.fn();

      const { container } = render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="hares"
          onBack={jest.fn()}
          onNavigateToLevel={onNavigateToLevel}
          onShowDrawer={jest.fn()}
        />,
      );

      // The current segment is a span, not a button, so clicking it should not navigate
      const currentSpan = container.querySelector('.breadcrumbCurrent');
      expect(currentSpan).not.toBeNull();
      fireEvent.click(currentSpan!);
      expect(onNavigateToLevel).not.toHaveBeenCalled();
    });
  });

  // AC3.8: stdlib read-only indicator
  describe('stdlib read-only indicator', () => {
    test('shows read-only badge for stdlib model', () => {
      const stack = [makeEntry('delay1', 'delay1')];

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="delay1"
          onBack={jest.fn()}
          onNavigateToLevel={jest.fn()}
          onShowDrawer={jest.fn()}
        />,
      );

      expect(screen.getByText('read-only')).not.toBeNull();
    });

    test('does not show read-only badge for non-stdlib model', () => {
      const stack = [makeEntry('hares', 'hares')];

      render(
        <BreadcrumbBar
          modelStack={stack}
          modelName="hares"
          onBack={jest.fn()}
          onNavigateToLevel={jest.fn()}
          onShowDrawer={jest.fn()}
        />,
      );

      expect(screen.queryByText('read-only')).toBeNull();
    });

    test('shows read-only badge for each of several stdlib models', () => {
      for (const stdlibModel of ['delay1', 'delay3', 'smth1', 'smth3', 'trend']) {
        const stack = [makeEntry(stdlibModel, stdlibModel)];

        const { unmount } = render(
          <BreadcrumbBar
            modelStack={stack}
            modelName={stdlibModel}
            onBack={jest.fn()}
            onNavigateToLevel={jest.fn()}
            onShowDrawer={jest.fn()}
          />,
        );

        expect(screen.getByText('read-only')).not.toBeNull();
        unmount();
      }
    });
  });

  // AC5.1: identical UI at any depth
  describe('consistent UI at different depths', () => {
    test('has same structural elements at depth 1 and depth 3', () => {
      const onBack = jest.fn();
      const onNavigateToLevel = jest.fn();
      const onShowDrawer = jest.fn();

      // Depth 1
      const stack1 = [makeEntry('hares', 'hares')];
      const { container: container1, unmount: unmount1 } = render(
        <BreadcrumbBar
          modelStack={stack1}
          modelName="hares"
          onBack={onBack}
          onNavigateToLevel={onNavigateToLevel}
          onShowDrawer={onShowDrawer}
        />,
      );

      const hasBackAt1 = container1.querySelector('[aria-label="Back"]') !== null;
      const hasSettingsAt1 = container1.querySelector('[aria-label="Model Properties"]') !== null;
      const hasBreadcrumbAt1 = container1.querySelector('.breadcrumb') !== null;

      unmount1();

      // Depth 3
      const stack3 = [
        makeEntry('population', 'population'),
        makeEntry('hares', 'hares'),
        makeEntry('sub_pop', 'sub_pop'),
      ];
      const { container: container3 } = render(
        <BreadcrumbBar
          modelStack={stack3}
          modelName="sub_pop"
          onBack={onBack}
          onNavigateToLevel={onNavigateToLevel}
          onShowDrawer={onShowDrawer}
        />,
      );

      const hasBackAt3 = container3.querySelector('[aria-label="Back"]') !== null;
      const hasSettingsAt3 = container3.querySelector('[aria-label="Model Properties"]') !== null;
      const hasBreadcrumbAt3 = container3.querySelector('.breadcrumb') !== null;

      // Both depths should have the same structural elements
      expect(hasBackAt1).toBe(true);
      expect(hasBackAt3).toBe(true);
      expect(hasSettingsAt1).toBe(true);
      expect(hasSettingsAt3).toBe(true);
      expect(hasBreadcrumbAt1).toBe(true);
      expect(hasBreadcrumbAt3).toBe(true);
    });
  });
});
