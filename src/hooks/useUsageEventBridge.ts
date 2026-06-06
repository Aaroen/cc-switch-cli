import { useEffect } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useQueryClient } from "@tanstack/react-query";
import { usageKeys } from "@/lib/query/usage";
import { isTauri } from "@/lib/platform/isTauri";
import { getPanelToken } from "@/lib/api/transport";

/**
 * 用量实时刷新桥接。收到后端"有新用量写入"的信号后立刻 invalidate UsageDashboard
 * 相关查询，让用户无需等待 30s 轮询周期。
 *
 * - 桌面端（Tauri）：监听 `usage-log-recorded` 事件。
 * - Web 控制台（浏览器）：订阅 SSE `/api/panel/usage-stream`（EventSource 无法设请求头，
 *   会话令牌经 `?token=` 传入）。这样网页端也能像 `tail -f` 日志一样近实时看到调用。
 *
 * 来源覆盖代理转发日志、Claude/Codex/Gemini 会话同步、启动归档。
 * 300ms 防抖合并，避免会话批量同步时的刷新风暴。该 hook 只挂在 UsageDashboard 上。
 */
export function useUsageEventBridge() {
  const queryClient = useQueryClient();

  useEffect(() => {
    let debounceTimer: ReturnType<typeof setTimeout> | undefined;
    const invalidate = () => {
      if (debounceTimer) return;
      debounceTimer = setTimeout(() => {
        debounceTimer = undefined;
        queryClient.invalidateQueries({ queryKey: usageKeys.all });
      }, 300);
    };

    // 桌面端：Tauri 事件通道
    if (isTauri()) {
      let unlisten: UnlistenFn | undefined;
      let disposed = false;
      (async () => {
        const off = await listen("usage-log-recorded", invalidate);
        if (disposed) {
          off();
        } else {
          unlisten = off;
        }
      })();
      return () => {
        disposed = true;
        if (debounceTimer) clearTimeout(debounceTimer);
        unlisten?.();
      };
    }

    // Web 控制台：SSE 推送（仅在已登录、拿到会话令牌时订阅）
    const token = getPanelToken();
    if (!token) {
      return () => {
        if (debounceTimer) clearTimeout(debounceTimer);
      };
    }
    const source = new EventSource(
      `/api/panel/usage-stream?token=${encodeURIComponent(token)}`,
    );
    source.addEventListener("usage", invalidate);
    // 连接中断时 EventSource 会自动重连；会话失效后由 AuthGate 卸载本组件并关闭连接。

    return () => {
      if (debounceTimer) clearTimeout(debounceTimer);
      source.close();
    };
  }, [queryClient]);
}
