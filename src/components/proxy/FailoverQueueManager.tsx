/**
 * 故障转移队列管理组件
 *
 * 允许用户管理代理模式下的故障转移队列，支持：
 * - 添加/移除供应商
 * - 队列顺序基于首页供应商列表的 sort_index
 */

import { useEffect, useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Plus, Trash2, Loader2, Info, AlertTriangle, Save } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import type { LoadBalanceStrategy } from "@/types/proxy";
import type { AppId } from "@/lib/api";
import type { Provider } from "@/types";
import { providersApi } from "@/lib/api/providers";
import { useProvidersQuery } from "@/lib/query/queries";
import { useAppProxyConfig } from "@/lib/query/proxy";
import { extractErrorMessage } from "@/utils/errorUtils";
import {
  useFailoverQueue,
  useAddToFailoverQueue,
  useRemoveFromFailoverQueue,
  useAutoFailoverEnabled,
  useSetAutoFailoverEnabled,
} from "@/lib/query/failover";

interface FailoverQueueManagerProps {
  appType: AppId;
  disabled?: boolean;
  weightRoundRobinEnabled?: boolean;
  loadBalanceStrategy?: LoadBalanceStrategy;
}

export function FailoverQueueManager({
  appType,
  disabled = false,
  weightRoundRobinEnabled,
  loadBalanceStrategy,
}: FailoverQueueManagerProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [weights, setWeights] = useState<Record<string, string>>({});

  // 故障转移开关状态（每个应用独立）
  const { data: isFailoverEnabled = false } = useAutoFailoverEnabled(appType);
  const setFailoverEnabled = useSetAutoFailoverEnabled();

  // 查询数据
  const {
    data: queue,
    isLoading: isQueueLoading,
    error: queueError,
  } = useFailoverQueue(appType);
  const { data: providersData, isLoading: isProvidersLoading } =
    useProvidersQuery(appType);
  const { data: proxyConfig } = useAppProxyConfig(appType);

  // Mutations
  const addToQueue = useAddToFailoverQueue();
  const removeFromQueue = useRemoveFromFailoverQueue();
  const updateWeight = useMutation({
    mutationFn: ({
      providerId,
      weight,
    }: {
      providerId: string;
      providerName: string;
      weight: number;
    }) => providersApi.updateWeight(providerId, appType, weight),
    onSuccess: async (_, variables) => {
      toast.success(
        t("proxy.weightRoundRobin.weightSaved", {
          provider: variables.providerName,
          weight: variables.weight,
          defaultValue: `${variables.providerName} 权重已保存`,
        }),
        { closeButton: true },
      );
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["providers", appType] }),
        queryClient.invalidateQueries({ queryKey: ["proxyStatus"] }),
      ]);
    },
    onError: (error: unknown) => {
      const detail =
        extractErrorMessage(error) ||
        t("common.unknown", { defaultValue: "未知错误" });
      toast.error(t("proxy.settings.toast.saveFailed", { error: detail }));
    },
  });

  const allProviders = Object.values(providersData?.providers ?? {});
  const effectiveWeightRoundRobinEnabled =
    weightRoundRobinEnabled ?? proxyConfig?.weightRoundRobinEnabled ?? false;
  const effectiveStrategy =
    loadBalanceStrategy ?? proxyConfig?.loadBalanceStrategy ?? "frequency";

  useEffect(() => {
    setWeights(
      Object.fromEntries(
        allProviders.map((provider) => [
          provider.id,
          String(getProviderWeight(provider)),
        ]),
      ),
    );
  }, [providersData]);

  const queueIndexByProviderId = useMemo(() => {
    return new Map(
      (queue ?? []).map((item, index) => [item.providerId, index] as const),
    );
  }, [queue]);

  // 切换故障转移开关
  const handleToggleFailover = (enabled: boolean) => {
    setFailoverEnabled.mutate({ appType, enabled });
  };

  // 添加供应商到队列
  const handleAddProvider = async (providerId: string) => {
    try {
      await addToQueue.mutateAsync({
        appType,
        providerId,
      });
      toast.success(
        t("proxy.failoverQueue.addSuccess", "已添加到故障转移队列"),
        { closeButton: true },
      );
    } catch (error) {
      toast.error(
        t("proxy.failoverQueue.addFailed", "添加失败") + ": " + String(error),
      );
    }
  };

  // 从队列移除供应商
  const handleRemoveProvider = async (providerId: string) => {
    try {
      await removeFromQueue.mutateAsync({ appType, providerId });
      toast.success(
        t("proxy.failoverQueue.removeSuccess", "已从故障转移队列移除"),
        { closeButton: true },
      );
    } catch (error) {
      toast.error(
        t("proxy.failoverQueue.removeFailed", "移除失败") +
          ": " +
          String(error),
      );
    }
  };

  const parseWeight = (value: string): number => {
    const trimmed = value.trim();
    if (!/^\d+$/.test(trimmed)) {
      return Number.NaN;
    }
    return Number.parseInt(trimmed, 10);
  };

  const totalWeight = allProviders.reduce((sum, provider) => {
    const weight = parseWeight(weights[provider.id] ?? "1");
    return sum + (Number.isNaN(weight) || weight <= 0 ? 0 : weight);
  }, 0);

  const metricLabel =
    effectiveStrategy === "weighted_random"
      ? t("proxy.weightRoundRobin.metricShare", { defaultValue: "流量占比" })
      : effectiveStrategy === "hard_round_robin"
        ? t("proxy.weightRoundRobin.metricRotation", { defaultValue: "轮转" })
        : t("proxy.weightRoundRobin.frequency", { defaultValue: "频率" });

  const weightHint =
    effectiveStrategy === "weighted_random"
      ? t("proxy.weightRoundRobin.providerWeightHintForward", {
          defaultValue:
            "输入 0-100 的整数。0 = 禁用；加权随机下数值越大流量占比越高。",
        })
      : effectiveStrategy === "hard_round_robin"
        ? t("proxy.weightRoundRobin.providerWeightHintEqual", {
            defaultValue:
              "输入 0-100 的整数。0 = 禁用；硬全轮询忽略权重大小，仅决定是否参与轮转。",
          })
        : t("proxy.weightRoundRobin.providerWeightHint", {
            defaultValue:
              "输入 0-100 的整数。0 = 禁用；频率控制按 1/N 分配轮询槽位，数值越小参与越频繁。",
          });

  const describeWeight = (weight: number): string => {
    if (weight === 0) {
      return t("proxy.weightRoundRobin.frequencyDisabled", {
        defaultValue: "已禁用",
      });
    }
    if (effectiveStrategy === "weighted_random") {
      if (totalWeight <= 0) {
        return "-";
      }
      return `${Math.round((weight / totalWeight) * 100)}%`;
    }
    if (effectiveStrategy === "hard_round_robin") {
      return t("proxy.weightRoundRobin.equalShare", { defaultValue: "等概率" });
    }
    if (weight === 1) {
      return t("proxy.weightRoundRobin.frequencyEveryRound", {
        defaultValue: "每轮",
      });
    }
    return t("proxy.weightRoundRobin.frequencyEveryN", {
      weight,
      defaultValue: `1/${weight}`,
    });
  };

  const handleSaveWeight = async (provider: Provider) => {
    const weight = parseWeight(weights[provider.id] ?? "1");
    if (Number.isNaN(weight) || weight < 0 || weight > 100) {
      toast.error(
        t("proxy.weightRoundRobin.validationFailed", {
          providers: provider.name,
          defaultValue: `以下供应商权重无效: ${provider.name}`,
        }),
      );
      return;
    }

    await updateWeight.mutateAsync({
      providerId: provider.id,
      providerName: provider.name,
      weight,
    });
  };

  if (isQueueLoading || isProvidersLoading) {
    return (
      <div className="flex items-center justify-center p-8">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (queueError) {
    return (
      <Alert variant="destructive">
        <AlertTriangle className="h-4 w-4" />
        <AlertDescription>{String(queueError)}</AlertDescription>
      </Alert>
    );
  }

  const queueLength = queue?.length ?? 0;

  return (
    <div className="space-y-4">
      {/* 自动故障转移开关 */}
      <div className="flex items-center justify-between p-4 rounded-lg bg-muted/50 border border-border/50">
        <div className="space-y-0.5">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium">
              {t("proxy.failover.autoSwitch", {
                defaultValue: "自动故障转移",
              })}
            </span>
            {isFailoverEnabled && (
              <span className="px-2 py-0.5 text-xs rounded-full bg-emerald-500/20 text-emerald-600 dark:text-emerald-400">
                {t("common.enabled", { defaultValue: "已开启" })}
              </span>
            )}
          </div>
          <p className="text-xs text-muted-foreground">
            {t("proxy.failover.autoSwitchDescription", {
              defaultValue:
                "开启后将立即切换到队列 P1，并在请求失败时自动切换到队列中的下一个供应商",
            })}
          </p>
        </div>
        <Switch
          checked={isFailoverEnabled}
          onCheckedChange={handleToggleFailover}
          disabled={disabled || setFailoverEnabled.isPending}
        />
      </div>

      {/* 说明信息 */}
      <Alert className="border-blue-500/40 bg-blue-500/10">
        <Info className="h-4 w-4" />
        <AlertDescription className="text-sm">
          {t(
            "proxy.failoverQueue.info",
            "队列顺序与首页供应商列表顺序一致。当请求失败时，系统会按顺序依次尝试队列中的供应商。",
          )}
        </AlertDescription>
      </Alert>

      {effectiveWeightRoundRobinEnabled && (
        <p className="text-xs text-muted-foreground">{weightHint}</p>
      )}

      {/* 供应商列表：故障转移与权重输入共用同一列表 */}
      {allProviders.length === 0 ? (
        <div className="rounded-lg border border-dashed border-muted-foreground/40 p-8 text-center">
          <p className="text-sm text-muted-foreground">
            {t(
              "proxy.failoverQueue.empty",
              "故障转移队列为空。添加供应商以启用自动故障转移。",
            )}
          </p>
        </div>
      ) : (
        <div className="space-y-2">
          {queueLength === 0 && (
            <p className="rounded-lg border border-dashed border-muted-foreground/30 px-3 py-2 text-xs text-muted-foreground">
              {t(
                "proxy.failoverQueue.empty",
                "故障转移队列为空。添加供应商以启用自动故障转移。",
              )}
            </p>
          )}
          {allProviders.map((provider) => (
            <ProviderRoutingItem
              key={provider.id}
              provider={provider}
              queueIndex={queueIndexByProviderId.get(provider.id)}
              disabled={disabled}
              weightRoundRobinEnabled={effectiveWeightRoundRobinEnabled}
              metricLabel={metricLabel}
              weightValue={weights[provider.id] ?? "1"}
              onWeightChange={(value) =>
                setWeights((prev) => ({ ...prev, [provider.id]: value }))
              }
              describeWeight={describeWeight}
              parseWeight={parseWeight}
              previousWeight={getProviderWeight(provider)}
              onRemove={handleRemoveProvider}
              onAdd={handleAddProvider}
              onSaveWeight={() => handleSaveWeight(provider)}
              isAdding={
                addToQueue.isPending &&
                addToQueue.variables?.providerId === provider.id
              }
              isRemoving={
                removeFromQueue.isPending &&
                removeFromQueue.variables?.providerId === provider.id
              }
              isSavingWeight={
                updateWeight.isPending &&
                updateWeight.variables?.providerId === provider.id
              }
            />
          ))}
        </div>
      )}

      {/* 队列说明 */}
      {queueLength > 0 && (
        <p className="text-xs text-muted-foreground">
          {t(
            "proxy.failoverQueue.orderHint",
            "队列顺序与首页供应商列表顺序一致，可在首页拖拽调整顺序。",
          )}
        </p>
      )}
    </div>
  );
}

function getProviderWeight(provider: Provider): number {
  return provider.weight ?? provider.meta?.routingWeight ?? 1;
}

interface ProviderRoutingItemProps {
  provider: Provider;
  queueIndex?: number;
  disabled: boolean;
  weightRoundRobinEnabled: boolean;
  metricLabel: string;
  weightValue: string;
  previousWeight: number;
  onWeightChange: (value: string) => void;
  describeWeight: (weight: number) => string;
  parseWeight: (value: string) => number;
  onAdd: (providerId: string) => void;
  onRemove: (providerId: string) => void;
  onSaveWeight: () => void;
  isAdding: boolean;
  isRemoving: boolean;
  isSavingWeight: boolean;
}

function ProviderRoutingItem({
  provider,
  queueIndex,
  disabled,
  weightRoundRobinEnabled,
  metricLabel,
  weightValue,
  previousWeight,
  onWeightChange,
  describeWeight,
  parseWeight,
  onAdd,
  onRemove,
  onSaveWeight,
  isAdding,
  isRemoving,
  isSavingWeight,
}: ProviderRoutingItemProps) {
  const { t } = useTranslation();
  const isInQueue = queueIndex !== undefined;
  const parsedWeight = parseWeight(weightValue);
  const isWeightInvalid =
    Number.isNaN(parsedWeight) || parsedWeight < 0 || parsedWeight > 100;
  const displayWeight = isWeightInvalid ? previousWeight : parsedWeight;
  const hasWeightChanged = !isWeightInvalid && parsedWeight !== previousWeight;

  return (
    <div
      className={cn(
        "grid gap-3 rounded-lg border bg-card p-3 transition-colors md:items-center",
        weightRoundRobinEnabled
          ? "md:grid-cols-[minmax(0,1fr)_112px_170px_120px]"
          : "md:grid-cols-[minmax(0,1fr)_112px]",
      )}
    >
      {/* 供应商名称 */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="truncate text-sm font-medium">{provider.name}</span>
          {isInQueue && (
            <Badge variant="secondary" className="shrink-0">
              P{queueIndex + 1}
            </Badge>
          )}
          {weightRoundRobinEnabled && displayWeight === 0 && (
            <Badge
              variant="secondary"
              className="shrink-0 border-transparent bg-rose-500/10 text-rose-600 dark:text-rose-300"
            >
              {t("proxy.weightRoundRobin.frequencyDisabled", {
                defaultValue: "已禁用",
              })}
            </Badge>
          )}
        </div>
        <p className="truncate text-xs text-muted-foreground">
          {provider.id}
          {provider.notes ? ` · ${provider.notes}` : ""}
        </p>
      </div>

      <Button
        variant={isInQueue ? "ghost" : "outline"}
        size="sm"
        className={cn(
          "w-full justify-center gap-1.5",
          isInQueue && "text-muted-foreground hover:text-destructive",
        )}
        onClick={() => (isInQueue ? onRemove(provider.id) : onAdd(provider.id))}
        disabled={disabled || isAdding || isRemoving}
      >
        {isAdding || isRemoving ? (
          <Loader2 className="h-4 w-4 animate-spin" />
        ) : isInQueue ? (
          <Trash2 className="h-4 w-4" />
        ) : (
          <Plus className="h-4 w-4" />
        )}
        {isInQueue
          ? t("proxy.failoverQueue.remove", { defaultValue: "移除" })
          : t("failover.addQueue", { defaultValue: "加入" })}
      </Button>

      {weightRoundRobinEnabled && (
        <div className="flex items-center gap-2">
          <Input
            type="number"
            min="0"
            max="100"
            value={weightValue}
            onChange={(event) => onWeightChange(event.target.value)}
            disabled={disabled || isSavingWeight}
            className={cn("h-8", isWeightInvalid && "border-red-500")}
            aria-label={t("proxy.weightRoundRobin.weight", {
              defaultValue: "权重",
            })}
          />
          <Button
            type="button"
            variant="outline"
            size="icon"
            className="h-8 w-8 shrink-0"
            onClick={onSaveWeight}
            disabled={
              disabled || isSavingWeight || isWeightInvalid || !hasWeightChanged
            }
            title={t("common.save", { defaultValue: "保存" })}
          >
            {isSavingWeight ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Save className="h-4 w-4" />
            )}
          </Button>
        </div>
      )}

      {weightRoundRobinEnabled && (
        <div className="space-y-1">
          <span className="text-xs font-medium text-muted-foreground">
            {metricLabel}
          </span>
          <Badge
            variant="secondary"
            className={
              displayWeight === 0
                ? "border-transparent bg-rose-500/10 text-rose-600 dark:text-rose-300"
                : "border-transparent bg-sky-500/10 text-sky-700 dark:text-sky-300"
            }
          >
            {describeWeight(displayWeight)}
          </Badge>
        </div>
      )}
    </div>
  );
}
