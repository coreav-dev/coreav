module.exports = async ({ github, context, core }) => {
  const { owner, repo } = context.repo;
  const repository = `${process.env.GITHUB_SERVER_URL || "https://github.com"}/${owner}/${repo}`;
  const run = `${repository}/actions/runs/${context.runId}`;
  if (context.eventName === "pull_request" && context.payload.pull_request.head.repo.full_name !== `${owner}/${repo}`) {
    const message = `Open a release-blocking issue for the failed WASM package gates: ${run}. This fork run has a read-only token.`;
    core.warning(message);
    await core.summary.addRaw(message).write();
    return;
  }
  const marker = `<!-- wasm-package-gates:${context.sha} -->`;
  const issues = await github.paginate(github.rest.issues.listForRepo, { owner, repo, state: "open", labels: "area:ci", per_page: 100 });
  if (issues.some((issue) => !issue.pull_request && issue.user?.login === "github-actions[bot]" && issue.title === `Release blocked: WASM package gates ${context.sha.slice(0, 12)}` && issue.body?.includes(marker))) return;
  await github.rest.issues.create({
    owner, repo, title: `Release blocked: WASM package gates ${context.sha.slice(0, 12)}`,
    labels: ["bug", "area:ci"],
    body: `${marker}\n\nThe installed-package and memory check failed at commit \`${context.sha}\`. This blocks release, not intervening merges.\n\nRun: ${run}\n\nPublic logs contain verdicts and registered bars only; raw memory records stay on the runner. A release requires the separately named verdict run at the frozen release-candidate SHA. A crash, timeout or startup failure is not automatically an infrastructure fault, and a missed bar must not be retuned or retried to overturn the result. See [the registered plan](${repository}/blob/${context.sha}/docs/design/wasm-memory-bars.md).`
  });
};
