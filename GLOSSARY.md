# Rustium

Rustium is a desktop editor for voxel scenes: people build Models out of Voxels, arrange them in a Scene and render it.

## Scene structure

**Scene**:
The document being edited: a tree of Nodes, one Palette and one Voxel size.
_Avoid_: project, world, file

**Node**:
One element of the Scene tree, either a Group or a Model.
_Avoid_: object, entity, item

**Group**:
A Node that holds other Nodes and moves them together.
_Avoid_: folder, layer

**Model**:
A Node that places a Shape in the Scene.
_Avoid_: object, mesh, volume

**Shape**:
The Voxels a Model shows; several Models may show the same Shape.
_Avoid_: model data, grid, mesh

**Instance**:
A Model whose Shape is shared with at least one other Model, so editing one edits all.
_Avoid_: copy, clone, link

**Transform**:
Where a Node sits relative to its parent: a whole-voxel offset plus a quarter-turn rotation or mirror.
_Avoid_: matrix, placement

**Volume**:
The box around a Shape's Voxels as shown in the editor; it grows when Voxels are attached outside it.
_Avoid_: bounds, size, canvas

**Voxel size**:
The real-world edge length of one Voxel, shared by the whole Scene.
_Avoid_: scale, resolution, unit

## Voxels and color

**Voxel**:
One cell of a Shape: either empty or showing one Material.
_Avoid_: block, cube, pixel

**Material**:
One entry of the Palette that says how a Voxel looks: color plus surface, glass, glow or haze properties.
_Avoid_: color, texture, paint

**Palette**:
The ordered list of up to 255 Materials a Scene can use.
_Avoid_: swatches, color table

**Color slot**:
One of the ten quick-pick entries on the hotbar, each pointing at a Material.
_Avoid_: favorite, hotkey color

## Editing

**Focus**:
The Node the user has stepped into; editing applies only inside it.
_Avoid_: active model, current object, target

**Tool**:
What a press in the viewport does: Select box, Select by color, Attach, Erase, Paint, Fill, Pick color, Move or Measure.
_Avoid_: mode, action

**Brush**:
The footprint of an Attach, Erase or Paint gesture: Voxel, Line, Box or Face.
_Avoid_: shape, stamp

**Mirror**:
Repeating every gesture reflected across chosen axes of the Focus.
_Avoid_: symmetry mode

**Selection**:
A set of Voxels inside the Focus that later Tools act on.
_Avoid_: marquee, region

**Command**:
A request to change the Scene, whether it comes from the interface, a script or an MCP client.
_Avoid_: action, operation, request

**Edit**:
One named change to the Scene that can be undone and redone as a unit.
_Avoid_: step, transaction, change

## Rendering

**Mode**:
The editor's top-level activity: Model (editing) or Render (making images).
_Avoid_: workspace, tab

**Shot**:
A saved camera together with the size and format of the image it produces.
_Avoid_: preset, view, bookmark

**Render style**:
The look a Shot is rendered in: Traced, Clay, Toon, Pixel or Wire.
_Avoid_: shader, filter

**Recovery file**:
The automatic background copy of a Scene that is offered back after a crash.
_Avoid_: backup, autosave file
