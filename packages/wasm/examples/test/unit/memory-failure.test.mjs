import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";

const report = createRequire(import.meta.url)("../../../../../.github/scripts/wasm-memory-failure.cjs");
const sha = "a".repeat(64);

function run(eventName = "push", repository = "coreav-dev/coreav", issues = []) {
  const created = [], warnings = [];
  const context = { repo: { owner: "coreav-dev", repo: "coreav" }, sha, runId: 123, eventName,
    payload: { pull_request: { head: { repo: { full_name: repository } } } } };
  const github = { rest: { issues: { listForRepo() {}, create: async (issue) => created.push(issue) } }, paginate: async () => issues };
  const core = { warning: (text) => warnings.push(text), summary: { addRaw(text) { warnings.push(text); return this; }, async write() {} } };
  return { github, context, core, created, warnings };
}

test("failed package gates create a public-safe release-blocking issue", async () => {
  for (const event of ["push", "pull_request", "workflow_dispatch"]) {
    const state = run(event);
    await report(state);
    assert.equal(state.created.length, 1);
    const issue = state.created[0];
    assert.match(issue.title, /^Release blocked:/);
    assert.deepEqual(issue.labels, ["bug", "area:ci"]);
    assert.match(issue.body, new RegExp(sha));
    assert.match(issue.body, /actions\/runs\/123/);
    assert.match(issue.body, /not intervening merges/);
    assert.doesNotMatch(issue.body, /memoryBytes|wasm_linear_bytes|heap|VmHWM/);
  }
});

test("the same head has one open release blocker while a new head does not reuse it", async () => {
  const issue = { user: { login: "github-actions[bot]" }, title: `Release blocked: WASM package gates ${sha.slice(0, 12)}`, body: `<!-- wasm-package-gates:${sha} -->` };
  const state = run("push", undefined, [issue]);
  await report(state);
  assert.equal(state.created.length, 0);
  state.context.sha = "b".repeat(64);
  await report(state);
  assert.equal(state.created.length, 1);
});

test("fork failures explicitly request a manual blocker without a write API call", async () => {
  const state = run("pull_request", "another/fork");
  state.github.paginate = () => assert.fail("forks must not write or look up issues");
  await report(state);
  assert.equal(state.created.length, 0);
  assert.match(state.warnings[0], /Open a release-blocking issue/);
});

test("an arbitrary issue cannot suppress the release blocker", async () => {
  const state = run("push", undefined, [{ body: `<!-- wasm-package-gates:${sha} -->` }]);
  await report(state);
  assert.equal(state.created.length, 1);
});

test("issue API failure is not treated as successful reporting", async () => {
  const state = run();
  state.github.rest.issues.create = async () => { throw new Error("unavailable"); };
  await assert.rejects(report(state), /unavailable/);
});
