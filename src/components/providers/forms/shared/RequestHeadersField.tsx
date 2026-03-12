import { useTranslation } from "react-i18next";
import { FormLabel } from "@/components/ui/form";
import JsonEditor from "@/components/JsonEditor";

interface RequestHeadersFieldProps {
  value: string;
  onChange: (value: string) => void;
  error?: string;
}

export function RequestHeadersField({
  value,
  onChange,
  error,
}: RequestHeadersFieldProps) {
  const { t } = useTranslation();

  return (
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
        {error ||
          t("providerForm.requestHeadersHint", {
            defaultValue:
              "这些请求头会在 CC Switch 调用上游模型服务时自动附加。",
          })}
      </p>
    </div>
  );
}
