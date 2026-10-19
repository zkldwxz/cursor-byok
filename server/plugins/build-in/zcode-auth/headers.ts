import type { ZCodeCredential } from "./resources.ts";

const OPENAI_AI_SDK_VERSION = "4.0.39";
const ANTHROPIC_AI_SDK_VERSION = "4.0.27";
const FALLBACK_NODE_MAJOR = "22";
const STAINLESS_PREFIX = "x-stainless-";

type RuntimeInfo = {
  category: "macos" | "windows" | "linux";
  arch: string;
  osVersion: string;
};

function userAgent(): string {
  return typeof navigator === "undefined" ? "" : navigator.userAgent;
}

function denoBuild(): { os?: string; arch?: string } | null {
  return (globalThis as { Deno?: { build?: { os?: string; arch?: string } } }).Deno?.build ??
    null;
}

function runtimeInfo(): RuntimeInfo {
  const build = denoBuild();
  const ua = userAgent();
  const lower = ua.toLowerCase();
  const os = build?.os ?? (lower.includes("mac") ? "darwin" : lower.includes("win")
    ? "windows"
    : "linux");
  const rawArch = build?.arch ?? (lower.includes("arm") ? "aarch64" : "x86_64");
  const category = os === "darwin" ? "macos" : os === "windows" ? "windows" : "linux";
  const arch = rawArch === "x86_64"
    ? "x64"
    : rawArch === "aarch64" || rawArch === "arm64"
    ? "arm64"
    : rawArch;
  const macVersion = /Mac OS X ([\d_]+)/.exec(ua)?.[1]?.replaceAll("_", ".");
  const winVersion = /Windows NT ([\d.]+)/.exec(ua)?.[1];
  return {
    category,
    arch,
    osVersion: macVersion ?? winVersion ?? "unknown",
  };
}

function clientLocale(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().locale || "en-US";
  } catch {
    return "en-US";
  }
}

function clientTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

function aiSdkVersion(data: ZCodeCredential): string {
  if (data.aiSdkVersion) return data.aiSdkVersion;
  return data.baseUrl.includes("/anthropic")
    ? ANTHROPIC_AI_SDK_VERSION
    : OPENAI_AI_SDK_VERSION;
}

/** Identity headers captured from the official ZCode desktop/CLI runtime. */
export function zcodeIdentityHeaders(data: ZCodeCredential): Record<string, string> {
  const platform = runtimeInfo();
  const locale = clientLocale();
  const timezone = clientTimezone();
  return {
    "User-Agent":
      `ZCode/${data.appVersion} ai-sdk/provider-utils/${aiSdkVersion(data)} runtime/node.js/${FALLBACK_NODE_MAJOR}`,
    "HTTP-Referer": "https://zcode.z.ai",
    "X-Title": `Z Code@${data.sourceTitle}`,
    "X-ZCode-App-Version": data.appVersion,
    "X-ZCode-Agent": "glm",
    "X-ZCode-Session-Type": "main",
    "X-Platform": `${platform.category === "macos" ? "darwin" : platform.category}-${platform.arch}`,
    "X-Os-Category": platform.category,
    "X-Os-Version": platform.osVersion,
    "X-Client-Language": locale,
    "X-Client-Timezone": timezone,
    "X-Release-Channel": data.releaseChannel,
    "X-Session-Id": data.sessionId,
    "X-Query-Id": crypto.randomUUID(),
    "X-Request-Id": crypto.randomUUID(),
    "X-ZCode-Trace-Id": crypto.randomUUID(),
  };
}


/** openai-node adds x-stainless-* fingerprints the official ZCode client never sends. */
export function stripSdkFingerprintHeaders(
  headers: Record<string, string>,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(headers).filter(([key]) => !key.toLowerCase().startsWith(STAINLESS_PREFIX)),
  );
}

export function zcodeRequestHeaders(
  base: Record<string, string>,
  data: ZCodeCredential,
): Record<string, string> {
  return {
    ...stripSdkFingerprintHeaders(base),
    ...zcodeIdentityHeaders(data),
  };
}
