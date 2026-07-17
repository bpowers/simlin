# simlin-site

Rspress-based documentation and website for simlin.com. Publishes the marketing homepage, docs, blog, and terms/privacy pages.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).

## Key Files

- `rspress.config.ts` -- Rspress site configuration (nav, sidebar, light-only mode, clean URLs)
- `docs/index.mdx` -- Homepage route stub (`pageType: home`); the actual homepage is the custom theme's `HomeLayout`
- `docs/docs/` -- User docs, served under the site's "docs" URL prefix (matching the deployed site's Docusaurus-era paths)
- `docs/blog/` -- Blog (an index page plus one page per post)
- `docs/terms.md`, `docs/privacy.md` -- Legal pages linked from the footer
- `docs/public/` -- Static assets (CNAME, robots.txt, logo, favicon)
- `theme/` -- Custom Rspress theme: `HomePage` (red hero + prose + live model + features), `SiteFooter` (dark multi-column footer rendered site-wide via the Layout `bottom` slot), and `PopulationModel`
- `theme/population.json` -- The homepage's logistic-growth model in Simlin's native JSON format
- `src/css/custom.css` -- Global styles (brand color, hero, features, footer)

## Model embedding

The homepage embeds a live model via `@simlin/diagram`'s `StaticDiagram` with `projectJson` + `simulate`: the WASM engine opens the native-JSON model in the browser, runs the base case, and attaches the results as sparklines. No precomputed series data is checked in; edit `theme/population.json` to change the model.

## Gotchas

- The site consumes the built workspace packages (`@simlin/core`, `@simlin/diagram`, `@simlin/engine` via `workspace:*`), so those must be built (`pnpm build`) before `rspress build`/`dev`.
- The SSG (node-target) bundle resolves `@simlin/diagram`'s Node build, whose `.css` files are CommonJS stubs; `rspress.config.ts` routes `diagram/lib/*.css` around the CSS pipeline (see the comments there). The web bundle resolves `lib.browser` and is unaffected.
- Internal links must be written WITHOUT trailing slashes ("docs", not "docs/"): `route.cleanUrls` rewrites a trailing-slash link into an ugly explicit "index" URL. The sidebar key (no trailing slash) prefix-matches all docs routes.
