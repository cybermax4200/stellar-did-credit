const { spawnSync } = require("node:child_process");

const ignoredAdvisories = new Set(["GHSA-vfj7-8cjw-p6xm"]);
const audit = spawnSync("pnpm audit --audit-level=high --json", {
  encoding: "utf8",
  shell: true,
});

if (audit.error) {
  console.error("Could not run pnpm audit:", audit.error.message);
  process.exit(1);
}

let report;
try {
  report = JSON.parse(audit.stdout);
} catch {
  console.error("pnpm audit did not return a valid JSON report.");
  if (audit.stderr) console.error(audit.stderr.trim());
  if (audit.stdout) console.error(audit.stdout.trim());
  process.exit(1);
}

const advisories = Object.values(report.advisories ?? {});
if (audit.status !== 0 && advisories.length === 0) {
  console.error("pnpm audit failed without returning advisory details.");
  if (audit.stderr) console.error(audit.stderr.trim());
  process.exit(1);
}

const blockingAdvisories = advisories.filter(
  ({ severity, github_advisory_id }) =>
    ["high", "critical"].includes(severity) &&
    !ignoredAdvisories.has(github_advisory_id),
);

if (blockingAdvisories.length > 0) {
  for (const advisory of blockingAdvisories) {
    console.error(
      `${advisory.severity}: ${advisory.module_name} ${advisory.github_advisory_id}`,
    );
  }
  process.exit(1);
}

if (
  advisories.some(({ github_advisory_id }) =>
    ignoredAdvisories.has(github_advisory_id),
  )
) {
  console.log(
    "Ignored GHSA-vfj7-8cjw-p6xm because braces has no patched release.",
  );
}
