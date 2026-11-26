//! Turns phone remote control on, pairs phones by QR code and lists those paired.
import { useCallback, useEffect, useState } from "react";
import { api, type PairingLink, type PairingQr, type RelaySettings, type RemoteStatus } from "../../shared/api";
import { Button } from "../../shared/ui/Button";
import { FormField, SecretTextInput, TextInput } from "../../shared/ui/FormControls";
import { Select } from "../../shared/ui/Select";
import { ConfirmDialog } from "../../shared/ui/ConfirmDialog";
import { Switch } from "../../shared/ui/Switch";
import { Tabs } from "../../shared/ui/Tabs";
import { TitledCard } from "../../shared/ui/TitledCard";
import { useMessage } from "../../shared/ui/message";
import styles from "./RemoteControlCard.module.scss";

const REFRESH_INTERVAL_MS = 3_000;

export function RemoteControlCard() {
  const message = useMessage();
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [pairing, setPairing] = useState<PairingLink | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const report = useCallback((cause: unknown) => message(cause instanceof Error ? cause.message : String(cause)), [message]);
  const enabled = status?.enabled ?? false;
  const devices = status?.devices ?? [];

  useEffect(() => {
    void api.remote().then(setStatus).catch(report);
  }, [report]);
  useEffect(() => {
    if (!enabled) {
      setPairing(null);
      return;
    }
    const timer = window.setInterval(() => void api.remote().then(setStatus).catch(report), REFRESH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [enabled, report]);
  // The code on screen is spent once a phone has used it.
  useEffect(() => setPairing(null), [devices.length]);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    try {
      await action();
    } catch (cause) {
      report(cause);
    } finally {
      setBusy(false);
    }
  };
  const turn = (next: boolean) => run(async () => setStatus(await api.setRemote(next)));
  const saveRelay = (next: RelaySettings) => run(async () => setStatus(await api.setRemoteRelay(next)));
  const remove = (id: string) => run(async () => {
    await api.removeRemoteDevice(id);
    setStatus(await api.remote());
  });

  return <TitledCard title={t("手机遥控")} action={enabled && <Button size="small" disabled={busy} onClick={() => void run(async () => setPairing(await api.pairRemote()))}>{t("配对手机")}</Button>}>
    <div className={styles.content}>
      <div className={styles.row}>
        <div className={styles.details}>
          <strong>{t("开启手机遥控")}</strong>
          <small>{t("用手机 App 或浏览器查看 Cursor 对话、发消息、批准操作和回答提问。App 的配对长期有效，重新开启不用再配；浏览器的地址每次都会变，要重新配对。不用的手机在下面移除。")}</small>
        </div>
        <Switch label={t("开启手机遥控")} checked={enabled} disabled={!status || busy} onChange={(next) => next ? setConfirming(true) : void turn(false)} />
      </div>
      {status && <RelayChoice status={status} busy={busy} onSave={saveRelay} />}
      {status && !status.cloudflared && <small className={styles.notice}>{t("没有找到 cloudflared，只能用 App 连接。要用浏览器请先安装。macOS：brew install cloudflared")}</small>}
      {status?.error && <small className={styles.notice}>{t("浏览器用的隧道没有连上，App 不受影响。要用浏览器请重新开启。")}</small>}
      {enabled && !status?.cursor && <small className={styles.notice}>
        {t("还没有连上 Cursor。在 Cursor 里打开 Agents Window，可以最小化，不要关闭；还连不上就重启一次 Cursor。")}
      </small>}
      {pairing && <Tabs items={[
        { value: "app", label: "App", content: <PairingCode qr={pairing.app} hint={t("在 Cursor BYOK App 里扫码。10 分钟内有效，只能配对一台手机；拿到它的人可以操作你的 Cursor，不要转发。")} /> },
        ...(pairing.web ? [{ value: "web", label: t("浏览器"), content: <PairingCode qr={pairing.web} hint={t("用手机相机扫码。10 分钟内有效，只能配对一台手机；拿到它的人可以操作你的 Cursor，不要转发。")} /> }] : []),
      ]} />}
      {devices.map((device) => <div className={styles.row} key={device.id}>
        <div className={styles.details}>
          <strong>{device.name}</strong>
          <small>{new Date(device.paired_at_ms).toLocaleString()}</small>
        </div>
        <Button size="small" disabled={busy} onClick={() => void remove(device.id)}>{t("移除")}</Button>
      </div>)}
    </div>
    <ConfirmDialog open={confirming} title={t("要开启手机遥控吗？")} busy={busy} cancelLabel={t("取消")} confirmLabel={t("开启")}
      onCancel={() => setConfirming(false)} onConfirm={() => void turn(true).then(() => setConfirming(false))}>
      {t("配对的手机可以让 Cursor 在这台电脑上运行命令。Cursor BYOK 接管 Cursor 期间，Cursor 自带的远程控制一直由 Cursor BYOK 应答。")}
    </ConfirmDialog>
  </TitledCard>;
}

type RelayMode = "public" | "custom" | "off";

const NO_RELAY: RelaySettings = { address: "", token: "" };

function modeOf(relay: RelaySettings, publicRelay: string): RelayMode {
  if (!relay.address) return "off";
  return relay.address === publicRelay && !relay.token ? "public" : "custom";
}

/** The public relay, one of the user's own, or none; a change counts from the next time remote control is turned on. */
function RelayChoice({ status, busy, onSave }: { status: RemoteStatus; busy: boolean; onSave: (relay: RelaySettings) => Promise<void> }) {
  const saved = modeOf(status.relay, status.public_relay);
  // What is typed for a relay of one's own, until it is saved.
  const [draft, setDraft] = useState<RelaySettings | null>(null);
  const mode = draft ? "custom" : saved;
  const choose = (next: string) => {
    if (next === "custom") return setDraft(saved === "custom" ? status.relay : NO_RELAY);
    setDraft(null);
    void onSave(next === "off" ? NO_RELAY : { address: status.public_relay, token: "" });
  };
  const notes: Record<RelayMode, string> = {
    public: t("手机和电脑不在同一网络时，由 BaoCode 的公共中继转发，它读不到对话内容。"),
    custom: t("用你自己的 iroh 中继转发，它读不到对话内容。"),
    off: t("不用中继，App 只能和电脑在同一网络时使用。"),
  };
  const shown = draft ?? status.relay;
  return <>
    <div className={styles.row}>
      <div className={styles.details}>
        <strong>{t("中继")}</strong>
        <small>{notes[mode]}{status.enabled ? t("关掉再打开手机遥控后生效。") : ""}</small>
      </div>
      <div className={styles.control}>
        <Select ariaLabel={t("中继")} value={mode} disabled={busy} onChange={choose} options={[
          { value: "public", label: t("默认（BaoCode）") },
          { value: "custom", label: t("自定义") },
          { value: "off", label: t("关闭") },
        ]} />
      </div>
    </div>
    {mode === "custom" && <form className={styles.relay} onSubmit={(event) => { event.preventDefault(); if (draft?.address.trim()) void onSave(draft).then(() => setDraft(null)); }}>
      <FormField label={t("中继地址")}>
        <TextInput value={shown.address} placeholder="https://relay.example.com" spellCheck={false} onChange={(event) => setDraft({ ...shown, address: event.target.value })} />
      </FormField>
      <FormField label={t("中继令牌")}>
        <SecretTextInput value={shown.token} autoComplete="off" onChange={(event) => setDraft({ ...shown, token: event.target.value })} />
      </FormField>
      <Button size="small" type="submit" disabled={busy || !draft?.address.trim()}>{t("保存")}</Button>
    </form>}
  </>;
}

function PairingCode({ qr, hint }: { qr: PairingQr; hint: string }) {
  return <div className={styles.pairing}>
    <img src={`data:image/svg+xml,${encodeURIComponent(qr.svg)}`} alt={t("配对二维码")} />
    <small>{hint}</small>
  </div>;
}
