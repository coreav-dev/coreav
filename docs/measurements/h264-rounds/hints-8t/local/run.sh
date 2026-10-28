#!/bin/bash
# Local timing A/B (information): dec-base = main 7af5d4f5, dec-k = the cut (EO). Eight threads unpinned, 6 rounds;
# one thread pinned, 4 rounds (bundle 2's ab8.py and ab1t.py with this VM's paths).
cd ~/c8/loc
python3 -I ab8.py 6 t8.json dec-base dec-k > t8.txt 2>&1
python3 -I ab1t.py 4 t1.json dec-base dec-k > t1.txt 2>&1
echo ALL >> t1.txt
