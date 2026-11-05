import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";

const report = createRequire(import.meta.url)("../../../../../.github/scripts/wasm-memory-failure.cjs");
const sha = "a".repeat(40);
const classes = ["installed", "examples", "installed_h264", "thumbnails"];

function run({ event = "push", ref = "refs/heads/main", issues = [], outcomes = { examples: "failure" } } = {}) {
  const created = [], comments = [];
  const context = { repo: { owner: "coreav-dev", repo: "coreav" }, sha, runId: 123, eventName: event, ref };
  const github = { rest: { issues: {
    listForRepo() {}, listComments() {},
    create: async (issue) => created.push(issue),
    createComment: async (comment) => comments.push(comment),
  } }, paginate: async (method) => method === github.rest.issues.listForRepo ? issues : [] };
  return { github, context, outcomes, created, comments };
}

function tracker(failureClass, number = 500) {
  const name = { installed: "installed package verification", examples: "example memory gates", installed_h264: "installed H.264 package verification", thumbnails: "thumbnail preflight" }[failureClass] ?? failureClass;
  return { number, user: { login: "github-actions[bot]" }, title: `Release blocked: WASM ${name}`,
    body: `<!-- wasm-package-gates:${failureClass} -->` };
}

test("main and explicit verdict runs create release blockers by failure class", async () => {
  for (const event of ["push", "workflow_dispatch"]) for (const failureClass of classes) {
    const state = run({ event, outcomes: { [failureClass]: "failure" } });
    await report(state);
    assert.equal(state.created.length, 1);
    const issue = state.created[0];
    assert.match(issue.title, /^Release blocked: WASM /);
    assert.doesNotMatch(issue.title, new RegExp(sha.slice(0, 12)));
    assert.deepEqual(issue.labels, ["bug", "area:ci", "tracking"]);
    assert.ok(issue.body.includes(`<!-- wasm-package-gates:${failureClass} -->`));
    assert.match(issue.body, new RegExp(sha));
    assert.match(issue.body, /actions\/runs\/123/);
    assert.match(issue.body, /not intervening merges/);
    assert.match(issue.body, /must not be retuned or retried/);
    assert.doesNotMatch(issue.body, /memoryBytes|wasm_linear_bytes|heap|VmHWM/);
  }
});

test("new failing commits comment on the same open class tracker", async () => {
  const state = run({ issues: [tracker("examples")] });
  await report(state);
  state.context.sha = "b".repeat(40);
  state.context.runId = 124;
  await report(state);
  assert.equal(state.created.length, 0);
  assert.deepEqual(state.comments.map(comment => comment.issue_number), [500, 500]);
  assert.match(state.comments[0].body, new RegExp(sha));
  assert.match(state.comments[1].body, new RegExp("b".repeat(40)));
  assert.match(state.comments[1].body, /actions\/runs\/124/);
});

test("a reporter retry does not repeat an existing run comment or initial issue report", async () => {
  const state = run({ issues: [tracker("examples")] });
  state.github.paginate = async method => method === state.github.rest.issues.listForRepo
    ? [tracker("examples")]
    : [{ user: { login: "github-actions[bot]" }, body: "<!-- wasm-package-gates-run:123:examples -->" }];
  await report(state);
  assert.equal(state.comments.length, 0);
  state.github.paginate = async () => [{ ...tracker("examples"), body: tracker("examples").body + "\n<!-- wasm-package-gates-run:123:examples -->" }];
  await report(state);
  assert.equal(state.comments.length, 0);
  assert.equal(state.created.length, 0);
});

test("PRs merge groups and non-main pushes never look up or write issues", async () => {
  for (const [event, ref] of [["pull_request", "refs/pull/1/merge"], ["merge_group", "refs/heads/gh-readonly-queue/main/test"], ["push", "refs/heads/fix"]]) {
    const state = run({ event, ref });
    state.github.paginate = () => assert.fail("this run is not a release-blocker source");
    await report(state);
    assert.equal(state.created.length + state.comments.length, 0);
  }
});

test("a manual candidate verdict need not be dispatched from main", async () => {
  const state = run({ event: "workflow_dispatch", ref: "refs/heads/candidate" });
  await report(state);
  assert.equal(state.created.length, 1);
});

test("successful skipped and cancelled steps do not file blockers", async () => {
  const state = run({ outcomes: { installed: "success", examples: "skipped", installed_h264: "cancelled" } });
  state.github.paginate = () => assert.fail("there is no failed gate to report");
  await report(state);
  assert.equal(state.created.length, 0);
});

test("unrelated legacy human and pull-request issues cannot absorb a class failure", async () => {
  for (const issue of [tracker("installed"), { ...tracker("examples"), user: { login: "someone" } },
    { ...tracker("examples"), pull_request: {} }, { ...tracker("examples"), title: "Unrelated" },
    { ...tracker(sha), title: `Release blocked: WASM package gates ${sha.slice(0, 12)}` }]) {
    const state = run({ issues: [issue] });
    await report(state);
    assert.equal(state.created.length, 1);
    assert.equal(state.comments.length, 0);
  }
});

test("the oldest matching tracker wins if duplicates already exist", async () => {
  const state = run({ issues: [tracker("examples", 502), tracker("examples", 501)] });
  await report(state);
  assert.equal(state.created.length, 0);
  assert.equal(state.comments[0].issue_number, 501);
});

test("issue lookup creation and comment failures remain reporting failures", async () => {
  for (const operation of ["lookup", "create", "comment"]) {
    const state = run({ issues: operation === "comment" ? [tracker("examples")] : [] });
    const fail = async () => { throw new Error("unavailable"); };
    if (operation === "lookup") state.github.paginate = fail;
    else if (operation === "create") state.github.rest.issues.create = fail;
    else state.github.rest.issues.createComment = fail;
    await assert.rejects(report(state), /unavailable/);
  }
});
