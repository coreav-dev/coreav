#!/bin/bash
# wait until the host gate passes twice in a row (at most 4 h), then start the one-thread run
for i in $(seq 48); do
  if python3 -I $HOME/hg/hostgate.py "wait" >> out/wait-gate.log 2>&1 && python3 -I $HOME/hg/hostgate.py "wait2" >> out/wait-gate.log 2>&1; then
    rm -f out/STOPPED; exec ./run-b4-1t.sh
  fi
  sleep 240
done
echo "host never settled in 4 h" > out/STOPPED
