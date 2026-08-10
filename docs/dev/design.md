# Product Design Context

Design context for user-facing surfaces: `src/diagram` (editor and component
library), `src/app` (the hosted application), `src/simlin-serve`'s web UI, and
`website`. Consult this when doing frontend or visual-design work.

## Users

System dynamics modelers and researchers who build stock-and-flow models. They come to Simlin to construct, simulate, and debug mental models of complex systems. The tool should feel like a natural extension of their thinking -- lowering barriers to SD modeling rather than adding cognitive overhead.

## Brand Personality

**Approachable, playful, modern.** Simlin should feel friendly and inviting, not intimidating or academic. The tagline "Debug your intuition" sets the tone: serious about insight, light about process.

## Aesthetic Direction

**Modern minimal** -- reduce visual weight, fewer shadows, flatter surfaces, generous whitespace. Inspired by **Figma and Linear**: clean professional tools with polished UX and obsessive attention to detail. Avoid dense IDE-like interfaces or cluttered dashboards.

The existing Material Design-inspired component library provides a solid foundation. Evolve it toward a lighter, more distinctive look: thinner borders, subtler elevation, more breathing room.

## Design Principles

1. **Clarity over decoration** -- Every visual element should serve comprehension. Remove what doesn't help the user think.
2. **Quiet until needed** -- Chrome and controls should recede. The model diagram is the primary artifact; UI supports it, not competes with it.
3. **Friendly precision** -- Warm and approachable, but never imprecise. Data and simulation results demand visual accuracy.
4. **Progressive disclosure** -- Simple by default, powerful on demand. Don't overwhelm new users; reward exploration for experts.
5. **Consistent and predictable** -- Follow established patterns from the component library. Spacing (8px grid), typography (Roboto), and color (primary #1976d2) should be applied uniformly.

## Design Tokens Reference

- **Primary**: #1976d2 | **Secondary**: #dc004e | **Selected**: #4444dd
- **Error**: #c62828 | **Success**: #2e7d32 | **Warning**: #f57f17
- **Font**: Roboto, Helvetica, Arial, sans-serif
- **Spacing base**: 8px grid
- **Border radius**: 4px (standard)
- **Dark mode**: Supported via `[data-theme="dark"]`
- **Component library**: `src/diagram/components/` (custom, no MUI)
- **Design tokens**: `src/diagram/theme.css` (the canonical token definitions; the hex values above are a quick reference, not a second source of truth)

## Accessibility

Best-effort approach: follow good practices (sufficient contrast, keyboard navigation, semantic HTML, focus indicators) without blocking on strict WCAG compliance.
