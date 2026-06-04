/**
 * 是否运行于 Tauri WebView。
 *
 * 用于在浏览器（CLI Web 控制台）下降级 Tauri 专属能力：
 * 传输层据此决定走原生 IPC 还是 HTTP 网关。
 */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
