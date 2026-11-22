//! Banner prompting to install/update the Cursor workspace-symbols extension, or the installed version line.
import { appStore, useAppStore } from "../../shared/store/appStore";
import { useMessage } from "../../shared/ui/message";
import controls from "../../shared/ui/Controls.module.scss";
import styles from "./CursorSettings.module.scss";
import { workspaceExtensionBanner } from "./workspaceExtension";

export function WorkspaceExtensionBanner() {
  const { cursorHarness, workspaceExtension, workspaceExtensionBusy, workspaceExtensionError, workspaceExtensionDismissed } = useAppStore();
  const message = useMessage();
  const takenOver = cursorHarness?.settings_applied ?? false;
  const banner = workspaceExtensionBanner(workspaceExtension, takenOver, workspaceExtensionDismissed);
  if (banner.kind === "none" && !workspaceExtensionError) return null;

  const saveVsix = async () => {
    const savedTo = await appStore.saveWorkspaceExtensionVsix();
    if (savedTo) message(t("VSIX 已保存到 {path}", { path: savedTo }));
  };
  const install = async () => {
    if (await appStore.installWorkspaceExtension()) {
      message(t("安装完成，重载 Cursor 窗口后生效"));
    }
  };

  const secondary = <button type="button" className={controls.secondary} disabled={workspaceExtensionBusy} onClick={() => void saveVsix()}>{t("另存 VSIX")}</button>;
  const dismiss = <button type="button" disabled={workspaceExtensionBusy} onClick={() => appStore.dismissWorkspaceExtension()}>{t("暂不")}</button>;

  if (banner.kind === "current") {
    return <div className={styles.extensionStatus}>
      <span>{t("工作区扩展 v{version} 已安装", { version: banner.version })}</span>
      <button type="button" disabled={workspaceExtensionBusy} onClick={() => void saveVsix()}>{t("另存 VSIX")}</button>
      {workspaceExtensionError && <span className={styles.extensionError}>{workspaceExtensionError}</span>}
    </div>;
  }

  return <div className={styles.extensionBanner}>
    {banner.kind === "missing" && <>
      <strong>{t("安装工作区搜索扩展")}</strong>
      <span>{t("让 Agent 按函数名和描述找代码、查引用")}</span>
      {workspaceExtension && !workspaceExtension.cursor_cli && <span className={styles.extensionError}>{t("未检测到 Cursor 命令行，请先手动安装扩展")}</span>}
      <div className={styles.extensionActions}>
        <button type="button" className={controls.primary} disabled={workspaceExtensionBusy} onClick={install}>{workspaceExtensionBusy ? t("安装中…") : t("安装")}</button>
        {secondary}
        {dismiss}
      </div>
    </>}
    {banner.kind === "outdated" && <>
      <strong>{t("更新工作区搜索扩展")}</strong>
      <span>{t("已安装 v{installed}，最新 v{expected}", {
        installed: banner.installed ?? "?",
        expected: banner.expected,
      })}</span>
      <div className={styles.extensionActions}>
        <button type="button" className={controls.primary} disabled={workspaceExtensionBusy} onClick={install}>{workspaceExtensionBusy ? t("安装中…") : t("更新")}</button>
        {secondary}
        {dismiss}
      </div>
    </>}
    {workspaceExtensionError && <span className={styles.extensionError}>{workspaceExtensionError}</span>}
    <span className={styles.extensionHint}>{t("远程服务器：本地装好后，在 Cursor 扩展视图对它点“Install in SSH: …”即可装到远端")}</span>
  </div>;
}
