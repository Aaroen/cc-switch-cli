import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { Loader2, LockKeyhole, ShieldCheck } from "lucide-react";
import {
  getWebPanelAuthState,
  loginWebPanel,
  setupWebPanelPassword,
} from "@/lib/api/webPanelAuth";
import {
  clearPanelToken,
  PANEL_AUTH_REQUIRED_EVENT,
} from "@/lib/api/transport";
import { isTauri } from "@/lib/platform/isTauri";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

type AuthMode = "loading" | "setup" | "login" | "ready" | "unavailable";

interface WebPanelAuthGateProps {
  children: ReactNode;
}

const MIN_PASSWORD_CHARS = 8;

export function WebPanelAuthGate({ children }: WebPanelAuthGateProps) {
  const [mode, setMode] = useState<AuthMode>(() =>
    isTauri() ? "ready" : "loading",
  );
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const refreshAuthState = async () => {
    setError(null);
    setMode("loading");
    try {
      const state = await getWebPanelAuthState();
      if (state.authenticated) {
        setMode("ready");
      } else if (state.setupRequired) {
        setMode("setup");
      } else {
        setMode("login");
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setMode("unavailable");
    }
  };

  useEffect(() => {
    if (isTauri()) return;

    void refreshAuthState();
    const handleAuthRequired = () => {
      clearPanelToken();
      setPassword("");
      setConfirmPassword("");
      setError("会话已失效，请重新登录");
      setMode("login");
    };
    window.addEventListener(PANEL_AUTH_REQUIRED_EVENT, handleAuthRequired);
    return () => {
      window.removeEventListener(PANEL_AUTH_REQUIRED_EVENT, handleAuthRequired);
    };
  }, []);

  if (mode === "ready") {
    return <>{children}</>;
  }

  const isSetup = mode === "setup";
  const title = isSetup ? "首次访问设置访问密码" : "登录 Web 控制面板";
  const description = isSetup
    ? "设置后，局域网内访问需要输入该密码。"
    : "请输入已设置的访问密码。";

  const validate = (): string | null => {
    if (!password.trim()) return "访问密码不能为空";
    if (Array.from(password).length < MIN_PASSWORD_CHARS) {
      return `访问密码至少需要 ${MIN_PASSWORD_CHARS} 个字符`;
    }
    if (isSetup && password !== confirmPassword) {
      return "两次输入的访问密码不一致";
    }
    return null;
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (mode !== "setup" && mode !== "login") return;

    const validationError = validate();
    if (validationError) {
      setError(validationError);
      return;
    }

    setSubmitting(true);
    setError(null);
    try {
      if (isSetup) {
        await setupWebPanelPassword(password);
      } else {
        await loginWebPanel(password);
      }
      setPassword("");
      setConfirmPassword("");
      setMode("ready");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="min-h-screen bg-background text-foreground flex items-center justify-center px-6 py-8">
      <div className="w-full max-w-sm rounded-lg border border-border bg-card p-6 shadow-sm">
        <div className="mb-6 flex flex-col items-center text-center">
          <div className="mb-4 grid h-12 w-12 place-items-center rounded-lg bg-primary/10 text-primary">
            {mode === "loading" ? (
              <Loader2 className="h-5 w-5 animate-spin" />
            ) : isSetup ? (
              <ShieldCheck className="h-5 w-5" />
            ) : (
              <LockKeyhole className="h-5 w-5" />
            )}
          </div>
          <h1 className="text-xl font-semibold tracking-normal">ccswicth</h1>
          <p className="mt-2 text-sm text-muted-foreground">{title}</p>
          {mode !== "loading" && mode !== "unavailable" && (
            <p className="mt-1 text-xs text-muted-foreground">{description}</p>
          )}
        </div>

        {mode === "loading" ? (
          <div className="h-24" />
        ) : mode === "unavailable" ? (
          <div className="space-y-4">
            {error && (
              <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {error}
              </div>
            )}
            <Button className="w-full" onClick={() => void refreshAuthState()}>
              重新连接
            </Button>
          </div>
        ) : (
          <form className="space-y-4" onSubmit={handleSubmit}>
            <div className="space-y-2">
              <Label htmlFor="web-panel-password">访问密码</Label>
              <Input
                id="web-panel-password"
                type="password"
                autoComplete={isSetup ? "new-password" : "current-password"}
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                disabled={submitting}
                autoFocus
              />
            </div>
            {isSetup && (
              <div className="space-y-2">
                <Label htmlFor="web-panel-password-confirm">确认密码</Label>
                <Input
                  id="web-panel-password-confirm"
                  type="password"
                  autoComplete="new-password"
                  value={confirmPassword}
                  onChange={(event) => setConfirmPassword(event.target.value)}
                  disabled={submitting}
                />
              </div>
            )}
            {error && (
              <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {error}
              </div>
            )}
            <Button className="w-full" type="submit" disabled={submitting}>
              {submitting && <Loader2 className="h-4 w-4 animate-spin" />}
              {isSetup ? "设置密码" : "登录"}
            </Button>
          </form>
        )}
      </div>
    </div>
  );
}
