#!/bin/bash
# Local timing A/B (information): dec-base = main 41c62136, dec-k = bundle 4 (db25cc6a). Eight threads unpinned, 6
# rounds; one thread pinned, 4 rounds (the banded hints' ab8.py and ab1t.py with this round's paths).
cd ~/b4/loc
python3 -I ab8.py 6 t8.json dec-base dec-k > t8.txt 2>&1
python3 -I ab1t.py 4 t1.json dec-base dec-k > t1.txt 2>&1
echo ALL >> t1.txt
