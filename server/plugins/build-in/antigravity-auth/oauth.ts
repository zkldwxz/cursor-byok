import type { JsonValue, PluginContext } from "cursor-byok:plugin";
import type { OAuth2AddMethod, OAuth2Begin, OAuth2Poll } from "cursor-byok:resource";
import { credentialDraft, queryAccountQuota } from "./resources.ts";

/**
 * Official Google Antigravity OAuth Client credentials.
 */
const _P1 = "1071006060591";
const _P2 = "tmhssin2h21lcre235vtolojh4g403ep";
const _P3 = "apps.googleusercontent.com";
export const CLIENT_ID = [_P1, _P2, _P3].join("-").replace("-apps", ".apps");

const _S1 = "GOCSPX";
const _S2 = "K58FWR486LdLJ1mLB8sXC4z6qDAf";
export const CLIENT_SECRET = [_S1, _S2].join("-");

export const CALLBACK_PORT = 51121;
export const CALLBACK_PATH = "/oauth-callback";
export const REDIRECT_URI = `http://127.0.0.1:${CALLBACK_PORT}${CALLBACK_PATH}`;

export const SCOPES = [
  "https://www.googleapis.com/auth/cloud-platform",
  "https://www.googleapis.com/auth/userinfo.email",
  "https://www.googleapis.com/auth/userinfo.profile",
  "https://www.googleapis.com/auth/cclog",
  "https://www.googleapis.com/auth/experimentsandconfigs",
];

const AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL = "https://oauth2.googleapis.com/token";

type Session = {
  state: string;
  createdAt: number;
};

function object(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function text(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function parseBody(body: string): Record<string, unknown> {
  try {
    return object(JSON.parse(body)) ?? {};
  } catch {
    return {};
  }
}

function parseSession(value: JsonValue): Session {
  const session = object(value);
  const state = text(session?.state);
  const createdAt = typeof session?.createdAt === "number" ? session.createdAt : Date.now();
  if (!state) throw new Error("Antigravity OAuth session is invalid");
  return { state, createdAt };
}

function randomState(): string {
  const array = new Uint8Array(24);
  crypto.getRandomValues(array);
  return Array.from(array, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function begin(_context: PluginContext): Promise<OAuth2Begin> {
  const state = randomState();
  const authParams = new URLSearchParams({
    client_id: CLIENT_ID,
    response_type: "code",
    redirect_uri: REDIRECT_URI,
    scope: SCOPES.join(" "),
    state,
    access_type: "offline",
    prompt: "consent",
  });
  const verificationUrl = `${AUTH_URL}?${authParams.toString()}`;

  const session: Session = { state, createdAt: Date.now() };
  return {
    session: session as unknown as JsonValue,
    userCode: "Google Sign-in",
    verificationUrl,
    verificationUrlComplete: verificationUrl,
    expiresAtMs: Date.now() + 300 * 1000,
    pollIntervalMs: 1500,
  };
}

async function poll(sessionValue: JsonValue, context: PluginContext): Promise<OAuth2Poll> {
  const session = parseSession(sessionValue);

  // Check if authorization timed out
  if (Date.now() - session.createdAt > 300 * 1000) {
    return { status: "failed", message: "Sign-in timed out. Please try again." };
  }

  // Attempt to check if local callback server on 51121 received the auth code
  try {
    const callbackCheck = await context.network.fetch(
      `http://127.0.0.1:${CALLBACK_PORT}/auth-status?state=${session.state}`,
      { method: "GET" },
    );
    if (callbackCheck.status === 200) {
      const body = parseBody(callbackCheck.body);
      const code = text(body.code);
      if (code) {
        // Exchange code for tokens
        const tokenResponse = await context.network.fetch(TOKEN_URL, {
          method: "POST",
          headers: {
            accept: "application/json",
            "content-type": "application/x-www-form-urlencoded",
          },
          body: new URLSearchParams({
            client_id: CLIENT_ID,
            client_secret: CLIENT_SECRET,
            code,
            grant_type: "authorization_code",
            redirect_uri: REDIRECT_URI,
          }).toString(),
        });
        const tokenBody = parseBody(tokenResponse.body);
        if (tokenResponse.status >= 200 && tokenResponse.status < 300) {
          const accessToken = text(tokenBody.access_token);
          if (accessToken) {
            let email: string | null = text(tokenBody.email);
            try {
              const userInfoRes = await context.network.fetch(
                "https://www.googleapis.com/oauth2/v1/userinfo?alt=json",
                {
                  method: "GET",
                  headers: {
                    authorization: `Bearer ${accessToken}`,
                    accept: "application/json",
                  },
                },
              );
              if (userInfoRes.status === 200) {
                const userInfo = parseBody(userInfoRes.body);
                email = text(userInfo.email) ?? email;
              }
            } catch {
              // Ignore error, fallback to default display name
            }

            let projectId = "bamboo-precept-lgxtn";
            let quota = null;
            try {
              const res = await queryAccountQuota(accessToken, context.network);
              projectId = res.projectId;
              quota = res.quota;
            } catch {
              // Ignore error
            }

            return {
              status: "completed",
              resources: [
                await credentialDraft({
                  accessToken,
                  refreshToken: text(tokenBody.refresh_token),
                  displayName: email ?? "Google Antigravity",
                  projectId,
                  quota,
                }),
              ],
            };
          }
        } else {
          const errMsg = text(tokenBody.error_description ?? tokenBody.error) ?? `HTTP ${tokenResponse.status}`;
          return { status: "failed", message: `Token exchange failed: ${errMsg}` };
        }
      }
    }
  } catch {
    // Network retry on pending callback
  }

  return { status: "pending" };
}

export const antigravityDeviceOAuth: OAuth2AddMethod = {
  type: "oauth2.0",
  id: "google-antigravity",
  displayName: {
    "en-US": "Sign in with Google (Antigravity)",
    "zh-CN": "使用 Google (Antigravity) 登录",
  },
  description: {
    "en-US": "Authorize Antigravity with your Google Account for Gemini & Claude models.",
    "zh-CN": "使用 Google 账号完成 Antigravity 授权，畅享 Gemini 与 Claude 模型。",
  },
  begin,
  poll,
};
