import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";

interface PipelineSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * A free-text setting field bound to a string pipeline setting. Keeps a local
 * draft so we only persist on blur / Enter (not on every keystroke), which
 * matters because the VAD-config setter rebuilds the audio recorder.
 */
const PipelineTextField: React.FC<{
  settingKey:
    | "pipeline_vad_config"
    | "pipeline_ffmpeg_args"
    | "pipeline_whisper_flags";
  title: string;
  description: string;
  placeholder: string;
  descriptionMode: "inline" | "tooltip";
  grouped: boolean;
}> = ({
  settingKey,
  title,
  description,
  placeholder,
  descriptionMode,
  grouped,
}) => {
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const persisted = (getSetting(settingKey) as string) ?? "";
  const [draft, setDraft] = useState(persisted);

  // Keep the draft in sync when the persisted value changes elsewhere.
  useEffect(() => {
    setDraft(persisted);
  }, [persisted]);

  const commit = () => {
    if (draft !== persisted) {
      updateSetting(settingKey, draft);
    }
  };

  return (
    <SettingContainer
      title={title}
      description={description}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="stacked"
    >
      <Input
        type="text"
        className="w-full font-mono"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          }
        }}
        placeholder={placeholder}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        disabled={isUpdating(settingKey)}
      />
    </SettingContainer>
  );
};

/**
 * The configurable whisper pipeline: one master toggle that gates three
 * free-text config stages (VAD/DSP, ffmpeg preprocessing, whisper decode
 * flags). When the toggle is off the app behaves exactly like upstream.
 */
export const PipelineSettings: React.FC<PipelineSettingsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("pipeline_enabled") || false;
    const vadEnabled = getSetting("pipeline_vad_enabled") ?? true;

    return (
      <>
        <ToggleSwitch
          checked={enabled}
          onChange={(value) => updateSetting("pipeline_enabled", value)}
          isUpdating={isUpdating("pipeline_enabled")}
          label={t("settings.advanced.pipeline.toggle.label")}
          description={t("settings.advanced.pipeline.toggle.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        {enabled && (
          <>
            <ToggleSwitch
              checked={vadEnabled}
              onChange={(value) => updateSetting("pipeline_vad_enabled", value)}
              isUpdating={isUpdating("pipeline_vad_enabled")}
              label={t("settings.advanced.pipeline.vadEnabled.label")}
              description={t("settings.advanced.pipeline.vadEnabled.description")}
              descriptionMode={descriptionMode}
              grouped={grouped}
            />
            {vadEnabled && (
              <PipelineTextField
                settingKey="pipeline_vad_config"
                title={t("settings.advanced.pipeline.vad.title")}
                description={t("settings.advanced.pipeline.vad.description")}
                placeholder="threshold=0.18 gain=12 prefill=20 hangover=25 onset=1"
                descriptionMode={descriptionMode}
                grouped={grouped}
              />
            )}
            <PipelineTextField
              settingKey="pipeline_ffmpeg_args"
              title={t("settings.advanced.pipeline.ffmpeg.title")}
              description={t("settings.advanced.pipeline.ffmpeg.description")}
              placeholder="-af loudnorm=I=-16:LRA=11:TP=-1.5"
              descriptionMode={descriptionMode}
              grouped={grouped}
            />
            <PipelineTextField
              settingKey="pipeline_whisper_flags"
              title={t("settings.advanced.pipeline.whisper.title")}
              description={t("settings.advanced.pipeline.whisper.description")}
              placeholder="-mc 0 --entropy-thold 2.8"
              descriptionMode={descriptionMode}
              grouped={grouped}
            />
          </>
        )}
      </>
    );
  },
);
