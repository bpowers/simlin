# system dynamics Model editor

[![Dependency Status](https://david-dm.org/bpowers/model-app/status.svg)](https://david-dm.org/bpowers/model-app)
[![devDependency Status](https://david-dm.org/bpowers/model-app/dev-status.svg)](https://david-dm.org/bpowers/model-app?type=dev)

Model is a tool for [System Dynamics modeling](https://www.systemdynamics.org/what-is-sd#overview).

![simple example model](doc/population-model.png)

## Controls

* Shift-click on the background to reposition the canvas (and drag selection doesn't currently do something).
* Click the blue edit button to show/hide new object creation tools.
  * use these tools by highlighting one of them and clicking on an existing item (in the case of "link" and maybe "flow"), or on the background of the canvas.
* Create a new flow by clicking-down on a stock (or on a blank spot on the canvas) and dragging.
* Detach + reposition/reattach flows + links by clicking-down on arrowheads and moving them over a new target.  The new target should highlight as green.
* Click on a variable name to edit the name.
* In equations, you can either refer to the variables like `"var name"` (with quotes), or like `var_name` (replacing spaces with underscores).
* The arrows in the bottom right are Undo and Redo - there is currently a 10-change limit for undos, and if you undo into the past + make a change (like moving something), you won't be able to redo-back those changes.

## Known issues

* Invalid equations aren't surfaced.
* The search box doesn't actually search/work yet.
* If you select a variable that is under the "search" box, the sheet showing variable details will cover that part of the model (you can manually shift-click the background and reposition the diagram to get out of/around this).
* Drag selection isn't implemented yet.
* You can only detach the arrowhead of a flow, not the origin/source end.
* Moving flows can be somewhat funky.
* Undo/redo only applies within a browser tab.  If you restart your browser (or reload the page), you will lose the ability to undo to before the reload.
* Only straight-line flows are supported for now.
* Its possible to (nonsensically) create multiple connectors between two variables.  Just maybe don't do that for now.
* Re-positioning labels isn't implemented yet.
* Units aren't implemented.
* Maybe more!  See the [Issues page](https://github.com/bpowers/model-app/issues) for additional info (or to highlight new bugs and problems)

## Local development

```bash
# dependencies; ignore warnings
$ yarn install
# start a local MongoDB instance using Docker
$ ./start-mongo
# in another tab:
$ . ~/model-oauth # (sets some environmental variables; get out of band)
$ yarn start:backend
# in third and final tab:
$ yarn start:frontend

```

Now to the browser!

Visit http://localhost:3030/auth/google to kick off an oauth request.  Once that is done, you can start local development + iteration on http://localhost:3000/ (any saves of the TypeScript files will automatically recompile + reload the service + React app).
