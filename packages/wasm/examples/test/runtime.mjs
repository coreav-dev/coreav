export class WorkerdDiagnostics {
  constructor(child, label) {
    this.child = child;
    this.label = label;
    this.stdout = [];
    this.stderr = [];
    this.launchError = null;
    this.startupFailure = null;
    this.closed = new Promise(resolve => child.once("close", resolve));
    child.on("error", error => { this.launchError = error; });
    child.stdout.setEncoding("utf8").on("data", chunk => this.stdout.push(chunk));
    child.stderr.setEncoding("utf8").on("data", chunk => this.stderr.push(chunk));
  }

  async assertOutputAbsent(pattern) {
    await this.closed;
    if (pattern.test(this.stdout.join("") + this.stderr.join(""))) throw new Error("workerd output contains a secret marker");
  }

  failure(reason) {
    const { exitCode, signalCode } = this.child;
    this.startupFailure = { reason, exitCode, signalCode, launchError: this.launchError?.code ?? null };
    return new Error(`workerd startup failed: stage=${this.label} reason=${reason} exit=${exitCode ?? "none"} signal=${signalCode ?? "none"}; process output is in .harness/runtime.log`);
  }

  record() {
    return { label: this.label, pid: this.child.pid ?? null, exitCode: this.child.exitCode, signalCode: this.child.signalCode,
      startupFailure: this.startupFailure, stdout: this.stdout.join(""), stderr: this.stderr.join("") };
  }
}
