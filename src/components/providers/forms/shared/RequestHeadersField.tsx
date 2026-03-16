import { useTranslation } from "react-i18next";
import { FormLabel } from "@/components/ui/form";
import JsonEditor from "@/components/JsonEditor";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { ProviderRequestHeadersAuthMode } from "@/types";

interface RequestHeadersFieldProps {
  value: string;
  onChange: (value: string) => void;
  error?: string;
  authMode?: ProviderRequestHeadersAuthMode;
  onAuthModeChange: (value?: ProviderRequestHeadersAuthMode) => void;
}

export function RequestHeadersField({
  value,
  onChange,
  error,
  authMode,
  onAuthModeChange,
}: RequestHeadersFieldProps) {
  const { t } = useTranslation();
  const authModeValue = authMode ?? "none";
  const helperText =
    authMode === "his_token"
      ? t("providerForm.requestHeadersHintHisToken", {
          defaultValue:
            "这些请求头会先作为 HIS token 脚本的入参；脚本返回的 JSON 才会真正写入上游请求。脚本文件会自动创建到应用配置目录的 scripts/provider-auth/his_token.js。",
        })
      : t("providerForm.requestHeadersHint", {
          defaultValue:
            "这些请求头会在 CC Switch 调用上游模型服务时自动附加。",
        });

  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <FormLabel htmlFor="requestHeadersAuthMode">
          {t("providerForm.requestHeadersAuthMode", {
            defaultValue: "鉴权模式",
          })}
        </FormLabel>
        <Select
          value={authModeValue}
          onValueChange={(nextValue) => {
            onAuthModeChange(
              nextValue === "his_token" ? "his_token" : undefined,
            );
          }}
        >
          <SelectTrigger id="requestHeadersAuthMode" className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="none">
              {t("providerForm.requestHeadersAuthModeNone", {
                defaultValue: "不启用",
              })}
            </SelectItem>
            <SelectItem value="his_token">
              {t("providerForm.requestHeadersAuthModeHisToken", {
                defaultValue: "HIS token",
              })}
            </SelectItem>
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">
          {t("providerForm.requestHeadersAuthModeHint", {
            defaultValue:
              "可选。启用后，请求头 JSON 会先作为脚本输入，再由脚本生成最终写入上游请求的请求头。",
          })}
        </p>
      </div>

      <div className="space-y-2">
        <FormLabel htmlFor="requestHeaders">
          {t("providerForm.requestHeaders", {
            defaultValue: "请求头 (JSON，可选)",
          })}
        </FormLabel>
        <JsonEditor
          id="requestHeaders"
          value={value}
          onChange={onChange}
          placeholder={`{
  "X-Provider-Token": "your-token",
  "HTTP-Referer": "https://example.com"
}`}
          rows={6}
          showValidation={true}
          language="json"
        />
        <p
          className={
            error ? "text-xs text-destructive" : "text-xs text-muted-foreground"
          }
        >
          {error || helperText}
        </p>
      </div>
    </div>
  );
}
