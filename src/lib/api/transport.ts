import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/platform/isTauri";

const TOKEN_KEY = "cc-switch-panel-token";

/**
 * 读取 Web 控制台访问令牌。
 *
 * 首次加载从 URL `?token=` 取出并存入 sessionStorage，随后从地址栏清除（避免泄漏到历史/日志），
 * 之后从 sessionStorage 读取。
 */
function panelToken(): string | null {
  if (typeof window === "undefined") return null;
  try {
    const url = new URL(window.location.href);
    const t = url.searchParams.get("token");
    if (t) {
      sessionStorage.setItem(TOKEN_KEY, t);
      url.searchParams.delete("token");
      window.history.replaceState({}, document.title, url.toString());
    }
  } catch {
    // 忽略 URL 解析失败
  }
  try {
    return sessionStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

/**
 * 统一 invoke。
 *
 * - Tauri 环境：走原生 IPC（`@tauri-apps/api/core`）。
 * - 浏览器（CLI Web 控制台）：走 `/api/invoke/:command` HTTP 网关，
 *   携带强制 Bearer 令牌与自定义头 `X-CC-Switch-Panel`。
 *
 * 与官方 invoke 签名一致，因此各 API 模块仅需将 import 来源改为本模块即可，无需改动业务/界面。
 */
export async function invoke<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (isTauri()) {
    return tauriInvoke<T>(cmd, args);
  }

  const token = panelToken();
  const res = await fetch(`/api/invoke/${encodeURIComponent(cmd)}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-CC-Switch-Panel": "1",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify(args ?? {}),
  });

  if (!res.ok) {
    throw new Error(`HTTP ${res.status}`);
  }

  const envelope = (await res.json()) as {
    ok: boolean;
    data?: T;
    error?: string;
  };
  if (envelope.ok) {
    return envelope.data as T;
  }
  // 与 Tauri invoke 语义对齐：以后端错误信息 reject
  throw new Error(envelope.error ?? "未知错误");
}
