//! Turns phone remote control on, pairs phones by QR code and lists those paired.
import { useCallback, useEffect, useState } from "react";
import { api, type PairingLink, type PairingQr, type RelaySettings, type RemoteStatus } from "../../shared/api";
import { Button } from "../../shared/ui/Button";
import { FormField, SecretTextInput, TextInput } from "../../shared/ui/FormControls";
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
  const [relay, setRelay] = useState<RelaySettings | null>(null);
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
  const saveRelay = (next: RelaySettings) => run(async () => {
    setStatus(await api.setRemoteRelay(next));
    setRelay(null);
  });
  const remove = (id: string) => run(async () => {
    await api.removeRemoteDevice(id);
    setStatus(await api.remote());
  });

  return <TitledCard title={t("手机遥控")} action={enabled && <Button size="small" disabled={busy} onClick={() => void run(async () => setPairing(await api.pairRemote()))}>{t("配对手机")}</Button>}>
    <div className={styles.content}>
      <div className={styles.row}>
        <div className={styles.details}>
          <strong>{t("开启手机遥控")}</strong>
          <small>{t("用手机 App 或浏览器查看 Cursor 对话、发消息、批准操作和回答提问。关闭或退出软件后，配对全部失效。")}</small>
        </div>
        <Switch label={t("开启手机遥控")} checked={enabled} disabled={!status || busy} onChange={(next) => next ? setConfirming(true) : void turn(false)} />
      </div>
      {status && !enabled && <form className={styles.relay} onSubmit={(event) => { event.preventDefault(); if (relay) void saveRelay(relay); }}>
        <FormField label={t("中继地址")} hint={t("手机和电脑不在同一网络时，由这台 iroh 中继转发，它读不到内容。留空则 App 只能在同一网络使用。")}>
          <TextInput value={(relay ?? status.relay).address} placeholder="https://relay.example.com" spellCheck={false} onChange={(event) => setRelay({ ...(relay ?? status.relay), address: event.target.value })} />
        </FormField>
        <FormField label={t("中继令牌")}>
          <SecretTextInput value={(relay ?? status.relay).token} autoComplete="off" onChange={(event) => setRelay({ ...(relay ?? status.relay), token: event.target.value })} />
        </FormField>
        <Button size="small" type="submit" disabled={busy || !relay}>{t("保存")}</Button>
      </form>}
      {status && !status.cloudflared && <small className={styles.notice}>{t("没有找到 cloudflared，只能用 App 连接。要用浏览器请先安装。macOS：brew install cloudflared")}</small>}
      {status?.error && <small className={styles.notice}>{t("浏览器用的隧道没有连上，App 不受影响。要用浏览器请重新开启。")}</small>}
      {enabled && !status?.cursor && <small className={styles.notice}>
        {t("还没有连上 Cursor。在 Cursor 里打开 Agents Window，可以最小化，不要关闭。首次开启后要重启两次 Cursor。")}
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
      {t("配对的手机可以让 Cursor 在这台电脑上运行命令。开启期间，Cursor 自带的远程控制由 Cursor BYOK 代为应答。")}
    </ConfirmDialog>
  </TitledCard>;
}

function PairingCode({ qr, hint }: { qr: PairingQr; hint: string }) {
  return <div className={styles.pairing}>
    <img src={`data:image/svg+xml,${encodeURIComponent(qr.svg)}`} alt={t("配对二维码")} />
    <small>{hint}</small>
  </div>;
}
