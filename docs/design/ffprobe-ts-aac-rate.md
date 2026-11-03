# Transport AAC analysis rates

The transport adapter reports the pinned FFmpeg 9.0.2 probe's running
integer mean of complete ADTS frame rates. ADTS supplies each frame's
length and sampling frequency (ISO/IEC 13818-7 section 6.2); the averaging
and analysis frontier are interoperability behavior, not a duration or
bit-rate field required by the transport specification.

## Plan

Keep the existing five-second media limit, five-million-byte payload
limit and independent eight-MiB transport head limit. Charge returned
packet payload lengths, not their physical transport offsets. Accumulate
known packet or configured picture periods, not timestamp gaps. The
initial PES configures a stream's clock with or without video; existing
single-frame initialization exceptions are not extended by this model.
Unknown codec periods remain unknown rather than receiving a guessed
duration.

Keep a stream's initial raw tail separate from its complete AAC frames.
When the packet listing begins with bytes before its first ADTS header,
the first complete frame initializes rate observation but does not enter
the integer mean. Later complete frames do, even when their packets have
no physical position. This eligibility state does not change packet
output or the media-duration clock.

When analysis closes,
the packet reader may already have parsed several complete audio frames
from the PES packet that completed a carried frame. Those queued headers
belong to the observation frontier even though the caller has not yet
received their packets.

Expose a borrowed iterator over already-buffered packets in return order.
It must not read, parse, allocate, clone payloads or consume the queue.
The reader retains ownership of every packet and its existing memory
limits. The adapter takes this view once, when analysis first closes,
and applies its existing complete-frame and split-frame checks to the
current packet and buffered packets within the head byte boundary when
they complete an already-admitted PES or a carried frame. A new,
unadmitted PES is not included merely because its packets were fetched.
Later metadata discovery cannot change the frozen rate. Other metadata,
packet output, timestamp selection and hard read limits remain unchanged.

Rejected: reading another PES packet, adjusting a cutoff to match an
input, or moving the probe's integer-average policy into the container
parser. A borrowed view represents work already performed without adding
per-stream statistics or another observation window.

## Verification

First establish a generated split-frame reproduction against pinned
9.0.2 and derive its integer mean from the generated frame inventory.
The CLI regression must fail before the fix. Check that complete queued
headers count once, split frames and incomplete successors do not count,
and later unparsed input cannot change the result. Include no-split,
audio-only and multiple-program controls. Verify that borrowing the
queue neither reads the source nor changes the subsequent packet list.
Vary the first two complete frame sizes independently with and without
leading bytes: only the first complete frame after a leading tail is
excluded. Vary the tail's length without changing that exclusion.

Run the transport tests, neighboring ffprobe tests, formatting and
targeted all-target Clippy. Recheck the identified development
disagreement without changing comparison rules or divergences. Do not
claim that other rate disagreements share this cause without checking
them, or that a development diagnostic is a judged result.
