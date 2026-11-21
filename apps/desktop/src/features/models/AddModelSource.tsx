//! Lets the user pick where a new Cursor model comes from: the local Magpie gateway or a manually configured provider.
import { useEffect, useState } from "react";
import { api, type MagpieCatalog } from "../../shared/api";
import { Icon } from "../../shared/ui/Icon";
import { chevronRightIcon, magpieIcon, tuneIcon } from "../../shared/ui/icons";
import { Modal } from "../../shared/ui/Modal";
import { useMessage } from "../../shared/ui/message";
import { enabledMagpieModels } from "./MagpieChannel";
import styles from "./AddModelSource.module.scss";

export function AddModelSource({ open, onClose, onMagpie, onCustom }: {
  open: boolean;
  onClose: () => void;
  onMagpie: () => void;
  onCustom: () => void;
}) {
  // undefined while loading; null when Magpie is not connected.
  const [catalog, setCatalog] = useState<MagpieCatalog | null | undefined>(undefined);
  const message = useMessage();

  useEffect(() => {
    if (!open) return;
    setCatalog(undefined);
    void api.magpie().then(setCatalog, (cause: unknown) => {
      message(cause instanceof Error ? cause.message : String(cause));
      setCatalog(null);
    });
  }, [open, message]);

  const magpieStatus = catalog === undefined
    ? <span className={styles.status}>{t("正在读取状态…")}</span>
    : catalog
      ? <span className={[styles.status, styles.connected].join(" ")}><i />{t("已连接 · 已启用 {enabled} / {total} 个模型", { enabled: enabledMagpieModels(catalog).length, total: catalog.models.length })}</span>
      : <span className={styles.status}>{t("未连接 · 一键发现本机网关")}</span>;

  return <Modal open={open} title={t("添加模型")} onClose={onClose} closeLabel={t("取消")}>
    <div className={styles.sources}>
      <button type="button" className={styles.source} onClick={onMagpie}>
        <span className={[styles.logo, styles.magpie].join(" ")}><Icon icon={magpieIcon} size="1.4em" /></span>
        <span className={styles.text}>
          <strong>{t("Magpie 本机网关")}</strong>
          <span>{t("自动同步本机 Magpie 的对话模型，无需填写地址和 API Key。")}</span>
          {magpieStatus}
        </span>
        <Icon icon={chevronRightIcon} size="1.2em" />
      </button>
      <button type="button" className={styles.source} onClick={onCustom}>
        <span className={styles.logo}><Icon icon={tuneIcon} size="1.4em" /></span>
        <span className={styles.text}>
          <strong>{t("自定义供应商")}</strong>
          <span>{t("手动填写 OpenAI 或 Anthropic 兼容接口的地址、API Key 和模型。")}</span>
        </span>
        <Icon icon={chevronRightIcon} size="1.2em" />
      </button>
    </div>
  </Modal>;
}
