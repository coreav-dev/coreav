Round 13, local direction (information only; the bars are read from the gating run).

Host: the 2.90 GHz class (Intel Xeon @ 2.90GHz, model 106), one core pinned (--pin 3).
Command, per class with the class's frame count (sd 1800, 720p 900, 1080p 300, 4k 150):
  scripts/h264_parity.py --pin 3 --rounds 3 --frames F --decoder base=... --decoder g=...
    --decoder r=... --decoder gr=... --ffmpeg <FFmpeg 9.0.2 BtbN n9.0.2-17-g2a571b6068> --report r13loc.CLASS.json
Builds (dist, rustc 1.99.1), all on main at b7cad07:
  base = b7cad07 (sha256 0908a1e0...), g = part G (9022e503...), r = part R on main (fc694fb0...),
  gr = both (4b5b35ed...).
analysis.txt is analyze.py's output on these reports (the bars are not read here).
