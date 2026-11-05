# Binding captured image timestamps

The image capture records two distinct clocks: the decoder's raw media PTS and the source's rational presentation time after edits. In ISO base media files, an edit list can map a nonzero media PTS to presentation time zero. The validator must not interpret both fields as edited timestamps.

For MOV/MP4 sources, use two independent probes of the same hashed bytes with the pinned FFprobe: its default edited timeline and its documented `ignore_editlist=1` media timeline. Bind corresponding decoded frames by their source packet byte positions, not by ordinal, a guessed constant offset or similar pixels. Require unique positions and raw timestamps, matching stream clocks and geometry, and complete correspondence for every presented source frame. Then bind the capture's raw PTS to the raw probe and its rational time to the edited probe of that same packet. Non-ISO sources retain their existing direct timestamp binding.

Retain both probe documents with the invocation evidence. Missing, ambiguous or inconsistent mapping remains a source-identity tooling fault; no frame is dropped to obtain a pass. Raw-PTS, rational-time and geometry mutations must still fail independently.

This corrects the implementation of the existing source-identity check. It does not change either exact pixel comparison, metadata acceptance, miss ordering, registration, bars, input roster or earlier records. The failed acquisition remains void. Verification uses mocked mappings and newly generated B-frame/edit-list sources only; another registered acquisition waits until this correction merges.

The [FFmpeg MOV demuxer documentation](https://ffmpeg.org/ffmpeg-formats.html#mov_002fmp4_002f3gp) states that the default demuxer reflects the timeline described by the edit list, while `ignore_editlist` ignores those atoms. CoreAV's `StreamPacket::pts` is in the track's media clock and `StreamPacket::time` includes the edit mapping; this correction preserves those existing semantics.
