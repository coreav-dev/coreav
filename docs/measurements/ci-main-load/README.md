# Main CI load sample

`sample.json` retains six completed successful main-push cohorts and two merge-group observations from November 1, 2026. Public commit SHAs identify the cohorts. Workflow creation times, observation times, and job names, states, conclusions and start times are retained; run identifiers, account metadata and job steps are omitted.

The sample is operational evidence, not a performance acceptance bar or a representative distribution. Count successful jobs in the completed main-push cohorts, excluding skipped jobs. For merge groups, use the earliest start of an in-progress or completed non-skipped job. GitHub also fills `started_at` on queued/skipped jobs; those values are not starts. When no job has started, report only a lower bound at the API request time.

From the repository root:

```sh
python3 -I - <<'PY'
import datetime
import json
import pathlib

data = json.loads(pathlib.Path("docs/measurements/ci-main-load/sample.json").read_text())
parse = lambda value: datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
for cohort in data["pushes"]:
    counts = {w["name"]: sum(j["conclusion"] == "success" for j in w["jobs"])
              for w in cohort["workflows"]}
    print("push", cohort["head_sha"][:12], counts, "total", sum(counts.values()))
for cohort in data["merge_groups"]:
    for workflow in cohort["workflows"]:
        starts = [parse(j["started_at"]) for j in workflow["jobs"]
                  if j["status"] in ("in_progress", "completed")
                  and j["conclusion"] != "skipped" and j["started_at"]]
        end = min(starts) if starts else parse(workflow["observed_at"])
        seconds = int((end - parse(workflow["created_at"])).total_seconds())
        minutes, seconds = divmod(seconds, 60)
        print("merge_group", cohort["head_sha"][:12], workflow["name"],
              "" if starts else ">=", f"{minutes}m{seconds:02}s")
PY
```

The six main-push totals are 34, 28, 7, 28, 28 and 7. The earlier merge group waited 9m45s–15m03s across its four workflows. In the later group Python started after 12m50s; the other three workflows were still queued after at least 15m07s at observation. Prior cancellation of queued main runs changed congestion during the sample, so it cannot establish a causal improvement.
