# Editing happens inside a Focus (Step in / Step out)

The user steps into one Node, its Focus; voxel Tools act only on the Model in Focus while the rest of the Scene stays visible and dimmed, and inside a Group the Tools select and move child Nodes. An earlier design let voxel Tools act on the whole Scene at once, which needed rules for which Model receives an Attach into empty space and for gestures crossing several Models; Focus removes those rules and matches the breadcrumb in the mockups.
