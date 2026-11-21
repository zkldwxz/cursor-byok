//! Connects the local Magpie gateway and chooses which of its chat models Cursor can use.
import { useEffect, useMemo, useState } from "react";
import { api, type MagpieCatalog } from "../../shared/api";
import { appStore } from "../../shared/store/appStore";
import controls from "../../shared/ui/Controls.module.scss";
import { Icon } from "../../shared/ui/Icon";
import { checkIcon, magpieIcon, refreshIcon, searchIcon } from "../../shared/ui/icons";
import { Modal } from "../../shared/ui/Modal";
import { useMessage } from "../../shared/ui/message";
import styles from "./MagpieChannel.module.scss";

type GatewayModel = MagpieCatalog["models"][number];

const errorText = (cause: unknown) => cause instanceof Error ? cause.message : String(cause);
export const enabledMagpieModels = (catalog: MagpieCatalog) => catalog.models.filter(model => !catalog.disabled.includes(model.id));
const modelName = (model: GatewayModel) => model.magpie_label ?? model.display_name ?? model.id;
const vendor = (id: string) => id.includes("/") ? id.slice(0, id.indexOf("/")) : "";

function contextLabel(tokens: number) {
  return tokens >= 1_000_000 ? `${+(tokens / 1_000_000).toFixed(1)}M` : `${Math.round(tokens / 1_000)}K`;
}

export function MagpieDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  // undefined while loading; null when Magpie is not connected.
  const [catalog, setCatalog] = useState<MagpieCatalog | null | undefined>(undefined);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const message = useMessage();

  const adopt = (next: MagpieCatalog | null, keep?: { previous: MagpieCatalog; selected: Set<string> }) => {
    setCatalog(next);
    if (!next) return setSelected(new Set());
    // A catalog sync keeps unsaved choices; models that just appeared follow the gateway's default.
    const known = new Set(keep?.previous.models.map(model => model.id));
    const enabled = new Set(enabledMagpieModels(next).map(model => model.id));
    setSelected(new Set(next.models.map(model => model.id)
      .filter(id => keep && known.has(id) ? keep.selected.has(id) : enabled.has(id))));
  };

  useEffect(() => {
    if (!open) return;
    setCatalog(undefined);
    setQuery("");
    void api.magpie().then(next => adopt(next)).catch(cause => { message(errorText(cause)); setCatalog(null); });
  }, [open]);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    try { await action(); } catch (cause) { message(errorText(cause)); } finally { setBusy(false); }
  };
  const connect = () => run(async () => {
    await api.connectMagpie();
    const previous = catalog;
    const next = await api.magpie();
    adopt(next, previous ? { previous, selected } : undefined);
    await appStore.refresh();
  });
  const save = () => run(async () => {
    await api.selectMagpieModels([...selected]);
    await appStore.refresh();
    onClose();
  });
  const disconnect = () => run(async () => {
    await api.disconnectMagpie();
    adopt(null);
    await appStore.refresh();
  });

  const groups = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const result = new Map<string, GatewayModel[]>();
    for (const model of catalog?.models ?? []) {
      if (needle && !`${model.id} ${modelName(model)}`.toLowerCase().includes(needle)) continue;
      const key = vendor(model.id);
      result.set(key, [...(result.get(key) ?? []), model]);
    }
    return [...result.entries()];
  }, [catalog, query]);

  const saved = catalog ? new Set(enabledMagpieModels(catalog).map(model => model.id)) : new Set<string>();
  const changed = selected.size !== saved.size || [...selected].some(id => !saved.has(id));
  const toggle = (ids: string[], on: boolean) => setSelected(current => {
    const next = new Set(current);
    for (const id of ids) {
      if (on) next.add(id);
      else next.delete(id);
    }
    return next;
  });

  if (catalog === undefined || catalog === null) {
    return <Modal open={open} title="Magpie" busy={busy} onClose={onClose} closeLabel={t("关闭")}
      submitLabel={t("连接 Magpie")} submitDisabled={catalog === undefined} onSubmit={() => void connect()}>
      {catalog === undefined ? <div className={styles.loading}>{t("正在读取 Magpie 状态…")}</div> : <div className={styles.intro}>
        <span className={styles.logo}><Icon icon={magpieIcon} size="1.6em" /></span>
        <strong>{t("连接本机 Magpie 网关")}</strong>
        <span>{t("先在这台电脑上启动 Magpie，BYOK 会自动找到它并同步可用的对话模型。")}</span>
        <ul>
          <li><Icon icon={checkIcon} size="1em" />{t("自动发现本机端口，无需填写 API Key")}</li>
          <li><Icon icon={checkIcon} size="1em" />{t("启动时和每 10 分钟同步一次模型目录")}</li>
          <li><Icon icon={checkIcon} size="1em" />{t("独立于手动添加的供应商，互不影响")}</li>
        </ul>
      </div>}
    </Modal>;
  }

  return <Modal open={open} wide title="Magpie" busy={busy} onClose={onClose} submitLabel={t("保存")} submitDisabled={!changed}
    onSubmit={() => void save()}
    secondaryAction={<button type="button" className={controls.secondary} disabled={busy} onClick={() => void disconnect()}>{t("断开连接")}</button>}>
    <div className={styles.panel}>
      <div className={styles.status}>
        <span className={styles.logo}><Icon icon={magpieIcon} size="1.3em" /></span>
        <div className={styles.statusText}>
          <strong><i className={styles.dot} />{t("已连接本机 Magpie")}</strong>
          <span>127.0.0.1:{catalog.port} · {t("已启用 {enabled} / {total} 个模型", { enabled: selected.size, total: catalog.models.length })}</span>
        </div>
        <button type="button" className={styles.textButton} disabled={busy} onClick={() => void connect()}>
          <Icon icon={refreshIcon} size="1em" />{t("同步目录")}
        </button>
      </div>

      <div className={styles.toolbar}>
        <label className={styles.search}>
          <Icon icon={searchIcon} size="1em" />
          <input value={query} placeholder={t("搜索模型名称或 ID")} aria-label={t("搜索模型名称或 ID")} onChange={event => setQuery(event.target.value)} />
        </label>
        <button type="button" className={styles.textButton} onClick={() => toggle(catalog.models.map(model => model.id), true)}>{t("全选")}</button>
        <button type="button" className={styles.textButton} onClick={() => toggle(catalog.models.map(model => model.id), false)}>{t("全不选")}</button>
      </div>

      {groups.length === 0 ? <div className={styles.loading}>{t("没有匹配的模型")}</div> : groups.map(([key, models]) => {
        const ids = models.map(model => model.id);
        const all = ids.every(id => selected.has(id));
        const count = ids.filter(id => selected.has(id)).length;
        return <section key={key} className={styles.group}>
          <div className={styles.groupHeader}>
            <span className={styles.vendor}>{key || t("其他")}</span>
            <span className={styles.count}>{count} / {ids.length}</span>
            <button type="button" className={styles.textButton} onClick={() => toggle(ids, !all)}>{all ? t("全不选") : t("全选")}</button>
          </div>
          {models.map(model => {
            const on = selected.has(model.id);
            const images = model.modalities?.input.includes("image");
            return <label key={model.id} className={[styles.model, on && styles.on].filter(Boolean).join(" ")}>
              <input type="checkbox" checked={on} onChange={event => toggle([model.id], event.target.checked)} />
              <span className={styles.box} aria-hidden>{on && <Icon icon={checkIcon} size="1em" />}</span>
              <span className={styles.name}>
                <span>{modelName(model)}</span>
                <small>{model.id}</small>
              </span>
              <span className={styles.badges}>
                {model.context_window ? <span>{contextLabel(model.context_window)}</span> : null}
                {model.reasoning && <span>{t("推理")}</span>}
                {images && <span>{t("图片")}</span>}
              </span>
            </label>;
          })}
        </section>;
      })}
    </div>
  </Modal>;
}
