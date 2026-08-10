# @simlin/app

Full-featured system dynamics application. Browse existing models, create or import new models, login/logout.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).
For product design context (users, brand, design principles, tokens), see [docs/dev/design.md](/docs/dev/design.md).

## Key Files

- `App.tsx` -- Root application component and routing. Surfaces /session exchange failures as `loginError` on the Login screen (the server answers a JSON `{error}` envelope; parsing is defensive so a proxy's non-JSON body can't make the failure look like a silent no-op, issue #927), and passes `user?.id` to `HostedWebEditor` as `authenticatedUserId` -- the change-signal that lets a deep link to a private project retry a load that raced session restoration (issue #933)
- `Home.tsx` -- Home/dashboard page
- `Login.tsx` -- Authentication UI
- `NewProject.tsx` -- New project creation flow
- `NewUser.tsx` -- New user onboarding
- `index.tsx` -- Application entry point
- `index-component.tsx` -- The `sd-model` custom element, embedding `HostedWebEditor` in a closed shadow root. Full lifecycle (issue #931): the shadow root attaches exactly once and is reused across reconnects (SPA hosts move nodes, firing disconnected/connected synchronously); disconnect unmounts the React root so the Editor disposes its controller and engine; a username/projectname attribute change while mounted is a project swap and remounts (a React root can't render after unmount, so every mount builds a fresh root); a repeat script evaluation is a no-op -- the first `customElements.define` wins, including its captured script-origin base URL
