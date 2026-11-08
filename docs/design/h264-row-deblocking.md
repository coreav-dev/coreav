# H.264 ordered row deblocking

The caller owns syntax output and the three reconstruction planes. Decision workers publish immutable, unfiltered macroblock candidates. Once a row is complete, the caller writes its syntax, commits its tiles, and filters that row while workers decide later rows. Prediction reads only published candidate tiles, never the filtered output planes.

Rows are consumed in raster order. Within each plane, each macroblock retains its vertical-then-horizontal edge order. Filtering a row can change the bottom border of the preceding row, but needs no samples from a later row. Cropping and reference publication happen only after every row is filtered and every worker has joined successfully.

The scheduler publishes each row outcome before sending a completion notification through a channel bounded by the picture height. Each worker owns its sender, and the caller drops the original sender before receiving. A failed or panicking decision propagates failure to dependent rows before its outcome is published. Every claimed row's compute callback stays inside the unwind boundary.

The caller remembers a consumer error or panic, stops consuming, and joins every worker. The first raster decision error takes precedence over a consumer error, preserving the earlier decide-all-before-writing failure order. A one-worker picture computes and consumes row by row without channels or threads, continuing decisions after a consumer failure for the same precedence. If no worker can start, the caller completes the decision work before consuming; partial starts retain the workers that did start.

The requested worker count still limits decision workers, with the existing geometry cap. The caller now does useful work concurrently but adds no operating-system thread. It can compete with decision workers on a limited CPU set, so the serial deblocking fraction is not a predicted speedup.

Band barriers would simplify completion ownership but add repeated spawning and idle boundaries. Moving mutable strips between workers would complicate border ownership while immutable candidates remain necessary. This design keeps the existing bounded decision table and picture allocations instead.

Verification compares complete streams and reconstructions with the post-ownership baseline at one, two, three and eight workers in automatic and scalar builds. Scheduler tests cover ordered overlap, decision and consumer failures and panics, raster error precedence, and zero or partial thread-start refusal. Row filtering is compared with whole-picture filtering on mixed edges and narrow pictures. Alternating explicit-SHA native reference calls at one and four workers must show an improvement beyond variation before this change ships.
