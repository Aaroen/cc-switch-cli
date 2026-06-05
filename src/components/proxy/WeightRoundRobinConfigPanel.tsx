import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Loader2, Info, Save } from "lucide-react";
import { toast } from "sonner";
import type { AppId } from "@/lib/api";
import type { LoadBalanceStrategy } from "@/types/proxy";
import { proxyApi } from "@/lib/api/proxy";
import { useAppProxyConfig } from "@/lib/query/proxy";
import { extractErrorMessage } from "@/utils/errorUtils";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";

interface WeightRoundRobinConfigPanelProps {
  appType: AppId;
  disabled?: boolean;
  onDraftChange?: (draft: {
    enabled: boolean;
    strategy: LoadBalanceStrategy;
  }) => void;
}

const STRATEGY_OPTIONS: Array<{
  value: LoadBalanceStrategy;
  labelKey: string;
  labelDefault: string;
  descKey: string;
  descDefault: string;
}> = [
  {
    value: "frequency",
    labelKey: "proxy.weightRoundRobin.strategy.frequency",
    labelDefault: "频率控制",
    descKey: "proxy.weightRoundRobin.strategy.frequencyDesc",
    descDefault: "基于硬全轮询按 1/N 频率分配槽位，权重越小参与轮询越频繁。",
  },
  {
    value: "weighted_random",
    labelKey: "proxy.weightRoundRobin.strategy.weightedRandom",
    labelDefault: "加权随机",
    descKey: "proxy.weightRoundRobin.strategy.weightedRandomDesc",
    descDefault: "权重越大流量越多，按权重占比随机分配；推荐多供应商分摊。",
  },
  {
    value: "hard_round_robin",
    labelKey: "proxy.weightRoundRobin.strategy.hardRoundRobin",
    labelDefault: "硬全轮询",
    descKey: "proxy.weightRoundRobin.strategy.hardRoundRobinDesc",
    descDefault: "权重 0 表示禁用，其余供应商忽略权重大小，按顺序等概率轮转。",
  },
];

export function WeightRoundRobinConfigPanel({
  appType,
  disabled = false,
  onDraftChange,
}: WeightRoundRobinConfigPanelProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const {
    data: config,
    isLoading: isConfigLoading,
    error: configError,
  } = useAppProxyConfig(appType);

  const [enabled, setEnabled] = useState(false);
  const [strategy, setStrategy] = useState<LoadBalanceStrategy>("frequency");

  useEffect(() => {
    if (config) {
      setEnabled(config.weightRoundRobinEnabled);
      setStrategy(config.loadBalanceStrategy ?? "frequency");
    }
  }, [config]);

  useEffect(() => {
    if (!config) {
      return;
    }
    onDraftChange?.({ enabled, strategy });
  }, [config, enabled, onDraftChange, strategy]);

  const saveMutation = useMutation({
    mutationFn: async ({
      nextEnabled,
      nextStrategy,
    }: {
      nextEnabled: boolean;
      nextStrategy: LoadBalanceStrategy;
    }) => {
      if (!config) {
        throw new Error(
          t("proxy.weightRoundRobin.configUnavailable", {
            defaultValue: "代理配置尚未加载完成",
          }),
        );
      }

      await proxyApi.updateProxyConfigForApp({
        ...config,
        weightRoundRobinEnabled: nextEnabled,
      });

      // 策略经专用命令写入（不经通用 update，避免回写覆盖）
      await proxyApi.setLoadBalanceStrategy(appType, nextStrategy);
    },
    onSuccess: async () => {
      toast.success(t("proxy.settings.toast.saved"), { closeButton: true });
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: ["appProxyConfig", appType],
        }),
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

  const handleSave = async () => {
    await saveMutation.mutateAsync({
      nextEnabled: enabled,
      nextStrategy: strategy,
    });
  };

  const handleReset = () => {
    if (config) {
      setEnabled(config.weightRoundRobinEnabled);
      setStrategy(config.loadBalanceStrategy ?? "frequency");
    }
  };

  if (isConfigLoading) {
    return (
      <div className="flex items-center justify-center p-4">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  const isDisabled = disabled || saveMutation.isPending;
  const errorMessage = extractErrorMessage(configError);
  const activeStrategy =
    STRATEGY_OPTIONS.find((option) => option.value === strategy) ??
    STRATEGY_OPTIONS[0];

  return (
    <div className="space-y-4">
      <div>
        <h4 className="text-sm font-semibold">
          {t("proxy.weightRoundRobin.title", {
            defaultValue: "权重轮询",
          })}
        </h4>
        <p className="text-xs text-muted-foreground">
          {t("proxy.weightRoundRobin.description", {
            defaultValue:
              "按供应商权重分配请求频率，并保留当前代理链路的故障转移能力。",
          })}
        </p>
      </div>

      {errorMessage && (
        <Alert variant="destructive">
          <AlertDescription>{errorMessage}</AlertDescription>
        </Alert>
      )}

      <div className="flex items-center justify-between rounded-lg border border-border/50 bg-muted/50 p-4">
        <div className="space-y-0.5">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium">
              {t("proxy.weightRoundRobin.enabled", {
                defaultValue: "启用权重轮询",
              })}
            </span>
            {enabled && (
              <span className="rounded-full bg-emerald-500/20 px-2 py-0.5 text-xs text-emerald-600 dark:text-emerald-400">
                {t("common.enabled", { defaultValue: "已启用" })}
              </span>
            )}
          </div>
          <p className="text-xs text-muted-foreground">
            {t("proxy.weightRoundRobin.enabledDescription", {
              defaultValue:
                "开启后，请求将优先按供应商权重分配；同一请求内仍保留故障转移回退。",
            })}
          </p>
        </div>
        <Switch
          checked={enabled}
          onCheckedChange={setEnabled}
          disabled={isDisabled}
          aria-label={t("proxy.weightRoundRobin.enabled", {
            defaultValue: "启用权重轮询",
          })}
        />
      </div>

      <div className="space-y-2 rounded-lg border border-border/50 bg-muted/50 p-4">
        <div className="flex items-center justify-between gap-4">
          <div className="space-y-0.5">
            <span className="text-sm font-medium">
              {t("proxy.weightRoundRobin.strategyTitle", {
                defaultValue: "轮询策略",
              })}
            </span>
            <p className="text-xs text-muted-foreground">
              {t(activeStrategy.descKey, {
                defaultValue: activeStrategy.descDefault,
              })}
            </p>
          </div>
          <Select
            value={strategy}
            onValueChange={(value) => setStrategy(value as LoadBalanceStrategy)}
            disabled={isDisabled || !enabled}
          >
            <SelectTrigger className="w-[200px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {STRATEGY_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {t(option.labelKey, { defaultValue: option.labelDefault })}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>

      <Alert className="border-blue-500/40 bg-blue-500/10">
        <Info className="h-4 w-4" />
        <AlertDescription className="text-sm">
          {strategy === "weighted_random"
            ? t("proxy.weightRoundRobin.infoForward", {
                defaultValue:
                  "加权随机：权重 0 表示禁用，数值越大被选中的流量占比越高（占比 = 权重 / 总权重）。",
              })
            : strategy === "hard_round_robin"
              ? t("proxy.weightRoundRobin.infoEqual", {
                  defaultValue:
                    "硬全轮询：权重 0 表示禁用，其余供应商忽略权重大小，按顺序等概率轮转。",
                })
              : t("proxy.weightRoundRobin.info", {
                  defaultValue:
                    "频率控制：在硬全轮询基础上按 1/N 分配轮询槽位。权重 0 表示禁用，数值越小参与轮询越频繁。",
                })}
        </AlertDescription>
      </Alert>

      <div className="flex justify-end gap-2">
        <Button variant="outline" onClick={handleReset} disabled={isDisabled}>
          {t("common.reset", { defaultValue: "重置" })}
        </Button>
        <Button onClick={handleSave} disabled={isDisabled}>
          {saveMutation.isPending ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Save className="mr-2 h-4 w-4" />
          )}
          {t("common.save", { defaultValue: "保存" })}
        </Button>
      </div>
    </div>
  );
}
