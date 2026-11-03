# Transport tail timestamps

A timestamp observed in the bounded tail is useful even when that window
does not establish the access unit's duration. The transport probe retains
that presentation timestamp independently of its end estimate. A known
end still keeps the timestamp paired with the selected access unit.

`coreav ffprobe` can complete an end estimate from a configured video
clock. For MPEG-2 at 25 frames per second, the final timestamp plus 3,600
ticks supplies the end; the stream duration subtracts its start. PES
timestamps use the 90 kHz clock of ISO/IEC 13818-1 section 2.4.3.7, and the
sequence's frame rate is defined by ISO/IEC 13818-2 section 6.3.3.

Retries remain driven by unresolved ends. A shorter window's known end
and its timestamp remain paired. Otherwise a longer window may establish
that pair, or contribute only a later observation; no-observation retries
cannot erase an already observed timestamp. No clock means no new end.

Regressions cover one timestamp in the bounded tail, empty retries,
resolution in a longer window and preservation of an existing pair.
Generated streams verify the CLI result against pinned FFmpeg 9.0.2.
Tail bounds, allocations, parsing, comparison rules and divergences remain
unchanged.
