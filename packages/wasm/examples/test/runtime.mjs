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
    child.stdout.on("data", chunk => this.stdout.push(chunk.toString()));
    child.stderr.on("data", chunk => this.stderr.push(chunk.toString()));
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
