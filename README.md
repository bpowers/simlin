# Simlin: a system dynamics modeling tool

Simlin is an [open source](LICENSE) tool for creating and editing [System Dynamics models](https://www.systemdynamics.org/what-is-sd#overview).  **You can try it today at https://simlin.com**.

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
* If you mouse-down and drag a label you can adjust its position relative to the variable.

## Known issues

* If you select a variable that is under the "search" box, the sheet showing variable details will cover that part of the model (you can manually shift-click the background and reposition the diagram to get out of/around this).
* Drag selection isn't implemented yet.
* You can only detach the arrowhead of a flow, not the origin/source end.
* Undo/redo only applies within a browser tab.  If you restart your browser (or reload the page), you will lose the ability to undo to before the reload.
* Only straight-line flows are supported for now.
* Maybe more!  See the [Issues page](https://github.com/bpowers/simlin/issues) for additional info (and to report a bug or usability issue)

## Local development

### Prerequisites

- **GCLOUD:** Make sure that you have installed the Google Cloud CLI and its firestore emulator. You can follow this [guide](https://cloud.google.com/sdk/docs/install-sdk#deb).

### Running the development version

```bash
# dependencies; ignore warnings
$ yarn install
# build everything -- necessary to compile the rust simulation engine to WebAssembly
$ yarn build
# (the next 3 commands don't exit on their own -- they should be run in 3 separate terminals)
# start a local Firestore instance using the Google Cloud SDK
$ yarn start:firestore
# in another tab:
$ yarn start:backend
# in third and final tab:
$ yarn start:frontend
```

Now to the browser!

You can start local development + iteration on http://localhost:3000/
