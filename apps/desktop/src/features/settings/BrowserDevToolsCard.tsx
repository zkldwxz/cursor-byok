//! Shows whether a browser is connected; the extension connects by itself.
import { useEffect, useState } from "react";
import { api } from "../../shared/api";
import { TitledCard } from "../../shared/ui/TitledCard";
import styles from "./CommitSettingsCard.module.scss";

const REFRESH_INTERVAL_MS = 3_000;

export function BrowserDevToolsCard() {
  const [connected, setConnected] = useState<boolean | null>(null);
  useEffect(() => {
    const refresh = () => void api.browserDevTools().then((status) => setConnected(status.connected), () => setConnected(null));
    refresh();
    const timer = window.setInterval(refresh, REFRESH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, []);
  return <TitledCard title={t("浏览器 DevTools")}>
    <div className={styles.content}>
      <div className={styles.row}>
        <div className={styles.details}>
          <strong>{t("Chrome 扩展")}</strong>
          <small>{t("装好扩展就会自动连接，不用配对。Cursor 会自己打开网页，或把你已打开的网页放进蓝色「Cursor」标签组；拖出标签组就收回了。")}</small>
        </div>
        <span className={styles.value}>{connected === null ? t("加载中…") : connected ? t("已连接") : t("未连接")}</span>
      </div>
    </div>
  </TitledCard>;
}
