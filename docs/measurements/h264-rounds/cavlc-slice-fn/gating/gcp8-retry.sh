#!/bin/bash
# gcp8-retry.sh NAME BASEBIN KBIN [extra]: gcp8.sh in us-east4-a / us-central1-a alternately, retrying every 5 minutes while the VM create fails with QUOTA_EXCEEDED (other sessions' VMs hold the slots); stops at 6 h.
NAME=$1; BB=$2; KB=$3; shift 3
end=$(( $(date +%s) + 21600 )); i=0
while [ $(date +%s) -lt $end ]; do
  zone=$([ $((i%2)) = 0 ] && echo us-east4-a || echo us-central1-a); i=$((i+1))
  ~/ab/scripts/gcp8.sh $NAME $BB $KB $zone "$@" > ~/ab/$NAME/gcp-try$i.log 2>&1; rc=$?
  if [ $rc != 4 ] || ! grep -q QUOTA_EXCEEDED ~/ab/$NAME/gcp-try$i.log; then echo "try $i zone $zone rc $rc $(date -u +%H:%M)" >> ~/ab/$NAME/gcp-tries.txt; exit $rc; fi
  echo "try $i zone $zone quota $(date -u +%H:%M)" >> ~/ab/$NAME/gcp-tries.txt; sleep 300
done
echo "gave up" >> ~/ab/$NAME/gcp-tries.txt; exit 7
