# H.264 row decision ownership

The row worker owns its input and prediction tiles until it publishes one complete macroblock decision. Neighbour reads continue to use immutable, published decisions. The scheduler releases the next row only after publication succeeds.

Construct the chroma tiles in their final arrays, fill all tiles through mutable references, and publish the selected candidate inside the row callback. The scheduler receives only success or failure instead of carrying the large candidate back across that callback boundary. Keep its row selection, dependency channels, panic handling and raster error order unchanged.

This removes temporary tile pairs and a candidate return without adding a per-macroblock allocation. It does not change prediction, search, coefficient refinement, entropy coding or deblocking. The picture-wide decision table and output-plane allocation remain unchanged.

An alternative is to allocate each candidate separately and move pointers between layers. That adds one allocation per macroblock and complicates allocation failure, so the first change retains the bounded picture table. Smaller tile layouts and persistent picture workspaces are separate work.

Verification compares complete encoded streams and reconstructions with the baseline at one, two, three and eight workers in automatic and scalar builds. Existing scheduler failure tests continue through a test-only value-returning adapter. Native x86 reference calls compare alternating baseline and candidate encodes at one and four workers; a gain must exceed variation before this change ships.
