module.exports = async ({ github, context, outcomes }) => {
  if (context.eventName !== "workflow_dispatch" && !(context.eventName === "push" && context.ref === "refs/heads/main")) return;
  const { owner, repo } = context.repo;
  const repository = `${process.env.GITHUB_SERVER_URL || "https://github.com"}/${owner}/${repo}`;
  const run = `${repository}/actions/runs/${context.runId}`;
  for (const [failureClass, name] of Object.entries({
    installed: "installed package verification",
    examples: "example memory gates",
    installed_h264: "installed H.264 package verification",
    thumbnails: "thumbnail preflight",
  })) {
    if (outcomes[failureClass] !== "failure") continue;
    const title = `Release blocked: WASM ${name}`;
    const marker = `<!-- wasm-package-gates:${failureClass} -->`;
    const runMarker = `<!-- wasm-package-gates-run:${context.runId}:${failureClass} -->`;
    const body = `${runMarker}\n\nThe ${name} failed at commit \`${context.sha}\`. This blocks release, not intervening merges.\n\nRun: ${run}\n\nPublic logs contain verdicts and registered bars only; raw memory records stay on the runner. A release requires the separately named verdict run at the frozen release-candidate SHA. A crash, timeout or startup failure is not automatically an infrastructure fault, and a missed bar must not be retuned or retried to overturn the result. See [the registered plan](${repository}/blob/${context.sha}/docs/design/wasm-memory-bars.md).`;
    const issues = await github.paginate(github.rest.issues.listForRepo, { owner, repo, state: "open", labels: "area:ci,tracking", per_page: 100 });
    const issue = issues.filter(issue => !issue.pull_request && issue.user?.login === "github-actions[bot]" && issue.title === title && issue.body?.includes(marker)).sort((a, b) => a.number - b.number)[0];
    if (!issue) {
      await github.rest.issues.create({ owner, repo, title, labels: ["bug", "area:ci", "tracking"], body: `${marker}\n${body}` });
      continue;
    }
    if (issue.body.includes(runMarker)) continue;
    const comments = await github.paginate(github.rest.issues.listComments, { owner, repo, issue_number: issue.number, per_page: 100 });
    if (comments.some(comment => comment.user?.login === "github-actions[bot]" && comment.body?.includes(runMarker))) continue;
    await github.rest.issues.createComment({ owner, repo, issue_number: issue.number, body });
  }
};
