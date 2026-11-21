import { useState } from "react";
import { api } from "../../shared/api";
import { Button } from "../../shared/ui/Button";
import { TextInput } from "../../shared/ui/FormControls";
import { TitledCard } from "../../shared/ui/TitledCard";
import { useMessage } from "../../shared/ui/message";

export function BrowserDevToolsCard() {
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const message = useMessage();
  const pair = async () => {
    setBusy(true);
    try {
      const { token } = await api.pairBrowserDevTools();
      const next = `${window.location.origin} ${token}`;
      setCode(next);
      await api.copyCursorText(next);
      message(t("已复制 Chrome 扩展配对码"));
    } catch (error) {
      message(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };
  return <TitledCard title={t("浏览器 DevTools")} action={<Button disabled={busy} onClick={() => void pair()}>{t("生成配对码")}</Button>}>
    <p>{t("把配对码粘贴到 Chrome 扩展，再共享要调试的标签页。重新配对会替换旧连接。")}</p>
    {code && <TextInput type="password" value={code} readOnly aria-label={t("Chrome 扩展配对码")} onFocus={(event) => event.currentTarget.select()} />}
  </TitledCard>;
}
