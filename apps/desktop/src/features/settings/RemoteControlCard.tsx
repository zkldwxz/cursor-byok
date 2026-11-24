//! Turns phone remote control on, pairs phones by QR code and lists those paired.
import { useCallback, useEffect, useState } from "react";
import { api, type PairingLink, type RemoteStatus } from "../../shared/api";
import { Button } from "../../shared/ui/Button";
import { ConfirmDialog } from "../../shared/ui/ConfirmDialog";
import { Switch } from "../../shared/ui/Switch";
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
  const remove = (id: string) => run(async () => {
    await api.removeRemoteDevice(id);
    setStatus(await api.remote());
  });

  return <TitledCard title={t("手机遥控")} action={enabled && <Button size="small" disabled={busy} onClick={() => void run(async () => setPairing(await api.pairRemote()))}>{t("配对手机")}</Button>}>
    <div className={styles.content}>
      <div className={styles.row}>
        <div className={styles.details}>
          <strong>{t("开启手机遥控")}</strong>
          <small>{t("通过临时 Cloudflare 隧道，用手机查看 Cursor 对话、发消息、批准操作和回答提问。关闭或退出软件后，地址和配对全部失效。")}</small>
        </div>
        <Switch label={t("开启手机遥控")} checked={enabled} disabled={!status || busy} onChange={(next) => next ? setConfirming(true) : void turn(false)} />
      </div>
      {status && !status.cloudflared && <small className={styles.notice}>{t("没有找到 cloudflared，请先安装。macOS：brew install cloudflared")}</small>}
      {status?.error && <small className={styles.notice}>{t("隧道已断开，请重新开启。")}</small>}
      {enabled && !status?.cursor && <small className={styles.notice}>
        {t("还没有连上 Cursor。在 Cursor 里打开 Agents Window，可以最小化，不要关闭。首次开启后要重启两次 Cursor。")}
      </small>}
      {pairing && <div className={styles.pairing}>
        <img src={`data:image/svg+xml,${encodeURIComponent(pairing.qr_svg)}`} alt={t("配对二维码")} />
        <small>{t("用手机相机扫码。10 分钟内有效，只能配对一台手机；拿到它的人可以操作你的 Cursor，不要转发。")}</small>
      </div>}
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
