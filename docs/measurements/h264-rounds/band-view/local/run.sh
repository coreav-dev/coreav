#!/bin/bash
# Local timing A/B (information): dec-base = main 82ad7c31, dec-k = this round. Eight threads unpinned, 6 rounds;
# one thread pinned, 4 rounds (bundle 4's ab8.py and ab1t.py with this round's paths).
cd ~/r5/w/loc
python3 -I ab8.py 6 t8.json dec-base dec-k > t8.txt 2>&1
python3 -I ab1t.py 4 t1.json dec-base dec-k > t1.txt 2>&1
echo ALL >> t1.txt
