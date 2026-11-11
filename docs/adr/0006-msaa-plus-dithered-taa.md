# MSAA for edges, dithering with TAA for glass and haze

The rasterized viewport uses MSAA (Off/2×/4×/8× in Settings) so voxel edges stay sharp, and draws transparent Materials (glass, haze) as stochastic dithering resolved by TAA instead of sorting or order-independent transparency. Sorting breaks on intersecting chunks, and weighted-blended OIT looks wrong for thick glass; the path tracer in Render mode is the reference for how transparency should look.
