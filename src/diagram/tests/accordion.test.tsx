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
  test('renders the summary as a button and exposes a region', () => {
    renderAccordion();
    const trigger = screen.getByRole('button', { name: /summary/i });
    expect(trigger).not.toBeNull();
    const region = screen.getByRole('region');
    expect(region.getAttribute('aria-labelledby')).toBe(trigger.getAttribute('id'));
    expect(trigger.getAttribute('aria-controls')).toBe(region.getAttribute('id'));
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
    renderAccordion();
    const region = screen.getByRole('region');
    // Collapsed: the content subtree is inert (not keyboard-reachable).
    expect(region.querySelector('[inert]')).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /summary/i }));
    // Open: the inert attribute is gone, so the content is interactive.
    expect(region.querySelector('[inert]')).toBeNull();
  });
});
