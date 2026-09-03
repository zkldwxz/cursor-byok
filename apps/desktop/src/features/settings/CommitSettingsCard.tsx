import { useCallback, useEffect, useMemo, useState } from "react";
import { api, type CommitSettingsView } from "../../shared/api";
import { useAppStore } from "../../shared/store/appStore";
import { Modal } from "../../shared/ui/Modal";
import { Select } from "../../shared/ui/Select";
import { TitledCard } from "../../shared/ui/TitledCard";
import { useMessage } from "../../shared/ui/message";
import controls from "../../shared/ui/Controls.module.scss";
import styles from "./CommitSettingsCard.module.scss";

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

export function CommitSettingsCard() {
  const { models } = useAppStore();
  const message = useMessage();
  const [view, setView] = useState<CommitSettingsView | null>(null);
  const [savingModel, setSavingModel] = useState(false);
  const [promptOpen, setPromptOpen] = useState(false);
  const [promptDraft, setPromptDraft] = useState("");
  const [savingPrompt, setSavingPrompt] = useState(false);

  useEffect(() => {
    void api.commitSettings().then(setView).catch((cause) => message(errorText(cause)));
  }, [message]);

  const modelOptions = useMemo(() => {
    const options: Array<{ value: string; label: string }> = [{ value: "", label: t("直连") }];
    const seen = new Set<string>();
    for (const model of models) {
      seen.add(model.model_hash);
      options.push({
        value: model.model_hash,
        label: model.display_name && model.display_name !== model.model_id
          ? `${model.display_name}（${model.model_id}）`
          : model.display_name || model.model_id,
      });
    }
    if (view?.model_id && !seen.has(view.model_id)) {
      options.push({ value: view.model_id, label: view.model_id });
    }
    return options;
  }, [models, view]);

  const selectedModelId = view?.model_id ?? "";

  const persist = useCallback(
    async (modelId: string, prompt: string) => {
      if (!view) return null;
      const normalizedPrompt =
        prompt.trim() === view.default_prompt.trim() ? "" : prompt.trim();
      return api.setCommitSettings({ model_id: modelId, prompt: normalizedPrompt });
    },
    [view],
  );

  const changeModel = useCallback(
    async (modelId: string) => {
      if (!view) return;
      setSavingModel(true);
      try {
        const saved = await persist(modelId, view.prompt);
        if (saved) setView(saved);
      } catch (cause) {
        message(errorText(cause));
      } finally {
        setSavingModel(false);
      }
    },
    [view, persist, message],
  );

  const openPrompt = useCallback(() => {
    if (!view) return;
    setPromptDraft(view.prompt || view.default_prompt);
    setPromptOpen(true);
  }, [view]);

  const savePrompt = useCallback(async () => {
    if (!view) return;
    setSavingPrompt(true);
    try {
      const saved = await persist(view.model_id, promptDraft);
      if (saved) setView(saved);
      setPromptOpen(false);
      message(t("提示词设置已保存"));
    } catch (cause) {
      message(errorText(cause));
    } finally {
      setSavingPrompt(false);
    }
  }, [view, promptDraft, persist, message]);

  const resetPrompt = useCallback(() => {
    if (!view) return;
    setPromptDraft(view.default_prompt);
  }, [view]);

  return (
    <>
      <TitledCard
        title={t("Commit 设置")}
        action={
          <button type="button" className={styles.textButton} disabled={!view} onClick={openPrompt}>
            {t("提示词设置")}
          </button>
        }
      >
        <div className={styles.row}>
          <div>
            <strong>{t("生成模型")}</strong>
            <small>{t("直连走 Cursor 原有通道；选择 Cursor 页已配置的模型后由本地生成。")}</small>
          </div>
          <div className={styles.select}>
            <Select
              value={selectedModelId}
              options={modelOptions}
              disabled={!view || savingModel}
              ariaLabel={t("生成模型")}
              onChange={(value) => void changeModel(value)}
            />
          </div>
        </div>
      </TitledCard>

      <Modal
        open={promptOpen}
        title={t("提示词设置")}
        wide
        fullHeight
        busy={savingPrompt}
        onClose={() => setPromptOpen(false)}
        onSubmit={() => void savePrompt()}
        submitLabel={t("保存")}
        closeLabel={t("取消")}
        secondaryAction={
          <button type="button" className={controls.secondary} onClick={resetPrompt}>
            {t("恢复默认")}
          </button>
        }
      >
        <textarea
          className={styles.promptEditor}
          value={promptDraft}
          spellCheck={false}
          aria-label={t("提交信息提示词")}
          onChange={(event) => setPromptDraft(event.target.value)}
        />
      </Modal>
    </>
  );
}
