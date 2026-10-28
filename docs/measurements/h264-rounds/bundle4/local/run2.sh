#!/bin/bash
# Local timing (information), second run: the first run's one-thread half overlapped a build (first/). Eight threads
# unpinned, base, H (the banded hints alone) and k, 10 rounds; one thread pinned, base and k, 6 rounds.
cd ~/b4/loc
python3 -I ab8.py 10 t8.json dec-base dec-H dec-k > t8.txt 2>&1
python3 -I ab1t.py 6 t1.json dec-base dec-k > t1.txt 2>&1
echo ALL >> t1.txt
