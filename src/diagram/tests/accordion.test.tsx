// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';
import { Accordion, AccordionSummary, AccordionDetails } from '../components/Accordion';

function renderAccordion(props: React.ComponentProps<typeof Accordion> = {}) {
  return render(
    <Accordion {...props}>
      <AccordionSummary expandIcon={<span data-testid="icon">v</span>}>Summary</AccordionSummary>
      <AccordionDetails>Details body</AccordionDetails>
    </Accordion>,
  );
}

describe('Accordion', () => {
  test('wires the region to the summary button', () => {
    // Collapsed, the region is inert (removed from the a11y tree), so query it
    // through the DOM rather than getByRole, which filters inaccessible nodes.
    const { container } = renderAccordion();
    const trigger = screen.getByRole('button', { name: /summary/i });
    const region = container.querySelector('[role="region"]');
    expect(region).not.toBeNull();
    expect(region?.getAttribute('aria-labelledby')).toBe(trigger.getAttribute('id'));
    expect(trigger.getAttribute('aria-controls')).toBe(region?.getAttribute('id'));
  });

  test('the region is hidden from assistive tech until expanded', () => {
    renderAccordion();
    // Collapsed: getByRole excludes the inert region.
    expect(screen.queryByRole('region')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /summary/i }));
    // Expanded: the region is exposed again.
    expect(screen.queryByRole('region')).not.toBeNull();
  });

  test('is collapsed by default and toggles open on click', () => {
    renderAccordion();
    const trigger = screen.getByRole('button', { name: /summary/i });
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
    expect(trigger.getAttribute('data-state')).toBe('closed');

    fireEvent.click(trigger);
    expect(trigger.getAttribute('aria-expanded')).toBe('true');
    expect(trigger.getAttribute('data-state')).toBe('open');
    expect(screen.getByRole('region').getAttribute('data-state')).toBe('open');
  });

  test('respects defaultExpanded for uncontrolled use', () => {
    renderAccordion({ defaultExpanded: true });
    expect(screen.getByRole('button', { name: /summary/i }).getAttribute('aria-expanded')).toBe('true');
  });

  test('controlled mode reflects the expanded prop and does not self-toggle', () => {
    const onChange = jest.fn();
    renderAccordion({ expanded: false, onChange });
    const trigger = screen.getByRole('button', { name: /summary/i });

    fireEvent.click(trigger);
    // Controlled: state only changes when the parent updates the prop.
    expect(onChange).toHaveBeenCalledWith(true);
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
  });

  test('disabled does not toggle or fire onChange', () => {
    const onChange = jest.fn();
    renderAccordion({ disabled: true, onChange });
    const trigger = screen.getByRole('button', { name: /summary/i });
    expect(trigger).toHaveProperty('disabled', true);

    fireEvent.click(trigger);
    expect(onChange).not.toHaveBeenCalled();
    expect(trigger.getAttribute('aria-expanded')).toBe('false');
  });

  test('renders the expand icon', () => {
    renderAccordion();
    expect(screen.getByTestId('icon')).not.toBeNull();
  });

  test('collapsed content is inert and becomes interactive when opened', () => {
    const { container } = renderAccordion();
    const region = container.querySelector('[role="region"]');
    // Collapsed: the region subtree is inert (not keyboard-reachable).
    expect(region?.hasAttribute('inert')).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: /summary/i }));
    // Open: the inert attribute is gone, so the content is interactive.
    expect(region?.hasAttribute('inert')).toBe(false);
  });
});
