import {
  clearPanelToken,
  getPanelToken,
  setPanelToken,
} from "@/lib/api/transport";

export interface WebPanelAuthState {
  setupRequired: boolean;
  authenticated: boolean;
}

interface TokenResponse {
  token: string;
}

interface ErrorResponse {
  error?: string;
}

async function readError(res: Response): Promise<string> {
  try {
    const payload = (await res.json()) as ErrorResponse;
    if (payload.error) return payload.error;
  } catch {
    // 忽略非 JSON 错误响应
  }
  return `HTTP ${res.status}`;
}

async function panelFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  const token = getPanelToken();
  const res = await fetch(path, {
    ...init,
    headers: {
      ...(init.body ? { "Content-Type": "application/json" } : {}),
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...init.headers,
    },
  });

  if (!res.ok) {
    if (res.status === 401) clearPanelToken();
    throw new Error(await readError(res));
  }

  return (await res.json()) as T;
}

export async function getWebPanelAuthState(): Promise<WebPanelAuthState> {
  return panelFetch<WebPanelAuthState>("/api/panel/auth-state");
}

export async function setupWebPanelPassword(password: string): Promise<void> {
  const payload = await panelFetch<TokenResponse>("/api/panel/setup", {
    method: "POST",
    body: JSON.stringify({ password }),
  });
  setPanelToken(payload.token);
}

export async function loginWebPanel(password: string): Promise<void> {
  const payload = await panelFetch<TokenResponse>("/api/panel/login", {
    method: "POST",
    body: JSON.stringify({ password }),
  });
  setPanelToken(payload.token);
}

export async function logoutWebPanel(): Promise<void> {
  try {
    await panelFetch<{ ok: boolean }>("/api/panel/logout", {
      method: "POST",
      body: JSON.stringify({}),
    });
  } finally {
    clearPanelToken();
  }
}
