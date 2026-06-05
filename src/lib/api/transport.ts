import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/platform/isTauri";

const TOKEN_KEY = "cc-switch-panel-session-token";
const LEGACY_TOKEN_KEY = "cc-switch-panel-token";
export const PANEL_AUTH_REQUIRED_EVENT = "cc-switch-panel-auth-required";

/**
 * 读取 Web 控制台会话令牌。
 */
export function getPanelToken(): string | null {
  if (typeof window === "undefined") return null;
  stripLegacyTokenFromUrl();
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function setPanelToken(token: string): void {
  if (typeof window === "undefined") return;
  try {
    localStorage.setItem(TOKEN_KEY, token);
    sessionStorage.removeItem(LEGACY_TOKEN_KEY);
  } catch {
    // 忽略存储失败，后续请求会按未登录处理
  }
}

export function clearPanelToken(): void {
  if (typeof window === "undefined") return;
  try {
    localStorage.removeItem(TOKEN_KEY);
    sessionStorage.removeItem(LEGACY_TOKEN_KEY);
  } catch {
    // 忽略存储清理失败
  }
}

function stripLegacyTokenFromUrl(): void {
  try {
    const url = new URL(window.location.href);
    if (!url.searchParams.has("token")) return;
    url.searchParams.delete("token");
    window.history.replaceState({}, document.title, url.toString());
  } catch {
    // 忽略 URL 解析失败
  }
}

function emitPanelAuthRequired(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new Event(PANEL_AUTH_REQUIRED_EVENT));
}

/**
 * 浏览器（Web 控制台）下硬不可实现的桌面专属命令。
 *
 * 这些命令依赖桌面端能力（原生对话框、外部应用/文件夹/终端、开机自启、应用更新器、
 * Tauri 托管的认证状态等），无服务端等价实现。命中时立即抛出清晰的本地化错误，
 * 避免落到网关产生不友好的"暂未支持"提示。前端对应入口亦应置灰（见 isTauri 判断）。
 */
const DESKTOP_ONLY_COMMANDS = new Set<string>([
  // 统一认证（Copilot/Codex OAuth）状态由 Tauri 托管，Web 面板无法共享运行实例
  "auth_get_status",
  "auth_list_accounts",
  "auth_logout",
  "auth_poll_for_account",
  "auth_remove_account",
  "auth_set_default_account",
  "auth_start_login",
  // 应用更新器 / 开机自启 / 进程控制
  "check_for_updates",
  "get_auto_launch_status",
  "set_auto_launch",
  "restart_app",
  // 打开外部应用/文件夹/终端（无法访问用户本机文件系统/终端）
  "launch_hermes_dashboard",
  "open_hermes_web_ui",
  "launch_session_terminal",
  "open_app_config_folder",
  "open_config_folder",
  "open_provider_terminal",
  "open_workspace_directory",
  // 原生目录选择（无法选择服务端任意目录）
  "pick_directory",
  "set_app_config_dir_override",
]);

const DESKTOP_ONLY_MESSAGE = "此功能仅在桌面端可用，Web 控制台不支持";

/**
 * 统一 invoke。
 *
 * - Tauri 环境：走原生 IPC（`@tauri-apps/api/core`）。
 * - 浏览器（CLI Web 控制台）：
 *   - `open_external`：用 `window.open` 在新标签打开 URL（浏览器等价能力）。
 *   - 硬桌面专属命令：抛出清晰错误（前端入口应已置灰）。
 *   - 其余：走 `/api/invoke/:command` HTTP 网关，携带强制 Bearer 令牌与自定义头。
 *
 * 与官方 invoke 签名一致，各 API 模块仅需将 import 来源改为本模块即可。
 */
export async function invoke<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (isTauri()) {
    return tauriInvoke<T>(cmd, args);
  }

  // 浏览器原生等价：在新标签打开外部链接
  if (cmd === "open_external") {
    const url = typeof args?.url === "string" ? (args.url as string) : "";
    if (url) window.open(url, "_blank", "noopener,noreferrer");
    return undefined as T;
  }

  // 硬桌面专属：立即给出清晰错误，避免落到网关
  if (DESKTOP_ONLY_COMMANDS.has(cmd)) {
    throw new Error(DESKTOP_ONLY_MESSAGE);
  }

  const token = getPanelToken();
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
    if (res.status === 401) {
      clearPanelToken();
      emitPanelAuthRequired();
      throw new Error("请重新登录 Web 控制台");
    }
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
