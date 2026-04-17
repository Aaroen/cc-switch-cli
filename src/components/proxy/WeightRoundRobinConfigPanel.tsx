import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Loader2, Info, Save } from "lucide-react";
import { toast } from "sonner";
import type { AppId } from "@/lib/api";
import { providersApi } from "@/lib/api/providers";
import { proxyApi } from "@/lib/api/proxy";
import { useProvidersQuery } from "@/lib/query/queries";
import { useAppProxyConfig } from "@/lib/query/proxy";
import { extractErrorMessage } from "@/utils/errorUtils";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";

interface WeightRoundRobinConfigPanelProps {
  appType: AppId;
  disabled?: boolean;
}

interface ProviderWeightUpdate {
  id: string;
  weight: number;
}

export function WeightRoundRobinConfigPanel({
  appType,
  disabled = false,
}: WeightRoundRobinConfigPanelProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const {
    data: config,
    isLoading: isConfigLoading,
    error: configError,
  } = useAppProxyConfig(appType);
  const {
    data: providersData,
    isLoading: isProvidersLoading,
    error: providersError,
  } = useProvidersQuery(appType);

  const [enabled, setEnabled] = useState(false);
  const [weights, setWeights] = useState<Record<string, string>>({});

  useEffect(() => {
    if (config) {
      setEnabled(config.weightRoundRobinEnabled);
    }
  }, [config]);

  useEffect(() => {
    const nextWeights = Object.fromEntries(
      Object.values(providersData?.providers ?? {}).map((provider) => [
        provider.id,
        String(provider.weight ?? provider.meta?.routingWeight ?? 1),
      ]),
    );
    setWeights(nextWeights);
  }, [providersData]);

  const providers = Object.values(providersData?.providers ?? {});

  const saveMutation = useMutation({
    mutationFn: async ({
      nextEnabled,
      updates,
    }: {
      nextEnabled: boolean;
      updates: ProviderWeightUpdate[];
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

      for (const update of updates) {
        await providersApi.updateWeight(update.id, appType, update.weight);
      }
    },
    onSuccess: async () => {
      toast.success(t("proxy.settings.toast.saved"), { closeButton: true });
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: ["appProxyConfig", appType],
        }),
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

  const parseWeight = (value: string): number => {
    const trimmed = value.trim();
    if (!/^\d+$/.test(trimmed)) {
      return Number.NaN;
    }
    return Number.parseInt(trimmed, 10);
  };

  const formatFrequency = (weight: number) => {
    if (weight === 0) {
      return t("proxy.weightRoundRobin.frequencyDisabled", {
        defaultValue: "已禁用",
      });
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

  const handleSave = async () => {
    const invalidProviders = providers.filter((provider) => {
      const weight = parseWeight(weights[provider.id] ?? "1");
      return Number.isNaN(weight) || weight < 0 || weight > 100;
    });

    if (invalidProviders.length > 0) {
      toast.error(
        t("proxy.weightRoundRobin.validationFailed", {
          providers: invalidProviders
            .map((provider) => provider.name)
            .join(", "),
          defaultValue: `以下供应商权重无效: ${invalidProviders
            .map((provider) => provider.name)
            .join(", ")}`,
        }),
      );
      return;
    }

    const updates = providers
      .map((provider) => ({
        id: provider.id,
        weight: parseWeight(weights[provider.id] ?? "1"),
        previousWeight: provider.weight ?? provider.meta?.routingWeight ?? 1,
      }))
      .filter((provider) => provider.weight !== provider.previousWeight)
      .map(({ id, weight }) => ({ id, weight }));

    await saveMutation.mutateAsync({
      nextEnabled: enabled,
      updates,
    });
  };

  const handleReset = () => {
    if (config) {
      setEnabled(config.weightRoundRobinEnabled);
    }

    setWeights(
      Object.fromEntries(
        providers.map((provider) => [
          provider.id,
          String(provider.weight ?? provider.meta?.routingWeight ?? 1),
        ]),
      ),
    );
  };

  if (isConfigLoading || isProvidersLoading) {
    return (
      <div className="flex items-center justify-center p-4">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  const isDisabled = disabled || saveMutation.isPending;
  const errorMessage =
    extractErrorMessage(configError) || extractErrorMessage(providersError);

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

      <Alert className="border-blue-500/40 bg-blue-500/10">
        <Info className="h-4 w-4" />
        <AlertDescription className="text-sm">
          {t("proxy.weightRoundRobin.info", {
            defaultValue:
              "权重 0 表示禁用该供应商，1 表示每轮都使用，2 表示每 2 轮使用一次。数值越小，请求频率越高。",
          })}
        </AlertDescription>
      </Alert>

      {providers.length === 0 ? (
        <div className="rounded-lg border border-dashed border-muted-foreground/40 p-8 text-center">
          <p className="text-sm text-muted-foreground">
            {t("proxy.weightRoundRobin.empty", {
              defaultValue: "当前应用暂无可配置的供应商。",
            })}
          </p>
        </div>
      ) : (
        <div className="space-y-4 rounded-lg border border-white/10 bg-muted/30 p-4">
          <div>
            <h5 className="text-sm font-semibold">
              {t("proxy.weightRoundRobin.providersTitle", {
                defaultValue: "供应商权重",
              })}
            </h5>
            <p className="text-xs text-muted-foreground">
              {t("proxy.weightRoundRobin.providerWeightHint", {
                defaultValue:
                  "输入 0-100 的整数。0 = 禁用，1 = 每轮都使用，数值越小频率越高。",
              })}
            </p>
          </div>

          <div className="space-y-3">
            {providers.map((provider) => {
              const currentWeight = parseWeight(weights[provider.id] ?? "1");
              const displayWeight = Number.isNaN(currentWeight)
                ? 1
                : currentWeight;

              return (
                <div
                  key={provider.id}
                  className="grid gap-3 rounded-lg border border-border/60 bg-background/60 p-3 md:grid-cols-[minmax(0,1fr)_104px_132px] md:items-center"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-sm font-medium">
                        {provider.name}
                      </span>
                      {displayWeight === 0 && (
                        <Badge
                          variant="secondary"
                          className="border-transparent bg-rose-500/10 text-rose-600 dark:text-rose-300"
                        >
                          {t("proxy.weightRoundRobin.frequencyDisabled", {
                            defaultValue: "已禁用",
                          })}
                        </Badge>
                      )}
                    </div>
                    <p className="truncate text-xs text-muted-foreground">
                      {provider.id}
                    </p>
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor={`weight-${appType}-${provider.id}`}>
                      {t("proxy.weightRoundRobin.weight", {
                        defaultValue: "权重",
                      })}
                    </Label>
                    <Input
                      id={`weight-${appType}-${provider.id}`}
                      type="number"
                      min="0"
                      max="100"
                      value={weights[provider.id] ?? "1"}
                      onChange={(event) =>
                        setWeights((prev) => ({
                          ...prev,
                          [provider.id]: event.target.value,
                        }))
                      }
                      disabled={isDisabled}
                    />
                  </div>

                  <div className="space-y-1">
                    <span className="text-xs font-medium text-muted-foreground">
                      {t("proxy.weightRoundRobin.frequency", {
                        defaultValue: "频率",
                      })}
                    </span>
                    <Badge
                      variant="secondary"
                      className={
                        displayWeight === 0
                          ? "border-transparent bg-rose-500/10 text-rose-600 dark:text-rose-300"
                          : "border-transparent bg-sky-500/10 text-sky-700 dark:text-sky-300"
                      }
                    >
                      {formatFrequency(displayWeight)}
                    </Badge>
                  </div>
                </div>
              );
            })}
          </div>

          <div className="flex justify-end gap-2">
            <Button
              variant="outline"
              onClick={handleReset}
              disabled={isDisabled}
            >
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
      )}
    </div>
  );
}
