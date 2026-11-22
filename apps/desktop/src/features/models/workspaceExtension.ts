//! Decides whether the Cursor page shows the workspace-extension banner or status line.
import type { WorkspaceExtensionStatus } from "../../shared/api";

export type WorkspaceExtensionBanner =
  | { kind: "none" }
  | { kind: "missing" }
  | { kind: "outdated"; installed: string | null; expected: string }
  | { kind: "current"; version: string };

export function workspaceExtensionBanner(
  status: WorkspaceExtensionStatus | null,
  takenOver: boolean,
  dismissed: boolean,
): WorkspaceExtensionBanner {
  if (!takenOver || !status) return { kind: "none" };
  if (status.state === "current") {
    return { kind: "current", version: status.installed_version ?? status.expected_version };
  }
  if (dismissed) return { kind: "none" };
  if (status.state === "missing") return { kind: "missing" };
  return { kind: "outdated", installed: status.installed_version, expected: status.expected_version };
}
