module.exports = async ({ github, context, outcomes }) => {
  if (context.eventName !== "workflow_dispatch" && !(context.eventName === "push" && context.ref === "refs/heads/main")) return;
  const { owner, repo } = context.repo;
  const repository = `${process.env.GITHUB_SERVER_URL || "https://github.com"}/${owner}/${repo}`;
  const run = `${repository}/actions/runs/${context.runId}`;
  for (const [failureClass, { name, plan }] of Object.entries({
    installed: { name: "installed package verification", plan: "wasm-memory-bars.md" },
    examples: { name: "example memory gates", plan: "wasm-memory-bars.md" },
    installed_h264: { name: "installed H.264 package verification", plan: "wasm-thumbnail-memory-bars.md" },
    thumbnails: { name: "thumbnail preflight", plan: "wasm-thumbnail-memory-bars.md" },
  })) {
    if (outcomes[failureClass] !== "failure") continue;
    const title = `Release blocked: WASM ${name}`;
    const marker = `<!-- wasm-package-gates:${failureClass} -->`;
    const runMarker = `<!-- wasm-package-gates-run:${context.runId}:${failureClass} -->`;
    const body = `${runMarker}\n\nThe ${name} failed at commit \`${context.sha}\`. This blocks release, not intervening merges.\n\nRun: ${run}\n\nPublic logs contain verdicts and registered bars only; raw memory records stay on the runner. A release requires the separately named verdict run at the frozen release-candidate SHA. A crash, timeout or startup failure is not automatically an infrastructure fault, and a missed bar must not be retuned or retried to overturn the result. See [the registered plan](${repository}/blob/${context.sha}/docs/design/${plan}).`;
    const query = { owner, repo, state: "open", labels: "area:ci,tracking", per_page: 100 };
    const matching = issues => issues.filter(issue => !issue.pull_request && issue.user?.login === "github-actions[bot]" && issue.title === title && issue.body?.includes(marker)).sort((a, b) => a.number - b.number);
    let issue = matching(await github.paginate(github.rest.issues.listForRepo, query))[0];
    let created;
    if (!issue) {
      ({ data: created } = await github.rest.issues.create({ owner, repo, title, labels: ["bug", "area:ci", "tracking"], body: `${marker}\n${body}` }));
      issue = matching([...await github.paginate(github.rest.issues.listForRepo, query), created])[0];
    }
    if (!issue.body.includes(runMarker)) {
      const comments = await github.paginate(github.rest.issues.listComments, { owner, repo, issue_number: issue.number, per_page: 100 });
      if (!comments.some(comment => comment.user?.login === "github-actions[bot]" && comment.body?.includes(runMarker))) {
        await github.rest.issues.createComment({ owner, repo, issue_number: issue.number, body });
      }
    }
    if (created && issue.number < created.number) {
      await github.rest.issues.createComment({ owner, repo, issue_number: created.number, body: `This failure is recorded in #${issue.number}.` });
      await github.rest.issues.update({ owner, repo, issue_number: created.number, state: "closed", state_reason: "not_planned" });
    }
  }
};
