import type { UseIntegrationsResult } from "../../hooks/useIntegrations";
import type { UseIntegrationFeaturesResult } from "../../hooks/useIntegrationFeatures";
import { useToast } from "../../hooks/useToast";
import SettingRow from "./SettingRow";
import Toggle from "./Toggle";

interface ContextTabProps {
  integrations: UseIntegrationsResult;
  features: UseIntegrationFeaturesResult;
}

export function ContextTab({ integrations, features }: ContextTabProps) {
  const { toast } = useToast();
  const {
    contextPreservation,
    contextPreservationInFlight,
    setContextPreservationEnabled,
    hasEnabledProvider,
    loading,
  } = integrations;

  const handleContextToggle = async (next: boolean) => {
    try {
      await setContextPreservationEnabled(next);
    } catch (err) {
      toast(
        "error",
        `${next ? "Enable" : "Disable"} failed for context preservation: ${String(err)}`,
      );
    }
  };

  const telemetryDisabled =
    !contextPreservation.enabled || features.saving || features.loading;

  const brevityEnabled = features.features.brevity;

  return (
    <div className="settings-panel">
      <SettingRow
        label="Working Context Preservation"
        description="Keep large transient context out of the active LLM transcript."
        control={
          <Toggle
            tone={
              contextPreservationInFlight
                ? "busy"
                : contextPreservation.enabled
                  ? "on"
                  : "off"
            }
            pressed={contextPreservation.enabled}
            disabled={contextPreservationInFlight || loading}
            onClick={() => void handleContextToggle(!contextPreservation.enabled)}
          />
        }
      />
      <SettingRow
        label="Context savings telemetry"
        description={
          contextPreservation.enabled
            ? "Records local-only metadata (event type, byte counts, refs — never the actual content) into the Quill database to power the Analytics → Context dashboard. In-session continuity keeps working when this is off; you just lose dashboard data for new sessions."
            : "Enable Working Context Preservation first. Telemetry rides with context preservation — without it there is nothing to record."
        }
        control={
          <Toggle
            tone={
              features.saving
                ? "busy"
                : !contextPreservation.enabled
                  ? "na"
                  : features.features.contextTelemetry
                    ? "on"
                    : "off"
            }
            label={
              features.saving
                ? "..."
                : !contextPreservation.enabled
                  ? "—"
                  : features.features.contextTelemetry
                    ? "ON"
                    : "OFF"
            }
            pressed={features.features.contextTelemetry}
            disabled={telemetryDisabled}
            onClick={() => {
              void features
                .setContextTelemetry(!features.features.contextTelemetry)
                .catch((e) => toast("error", String(e)));
            }}
          />
        }
      />
      <div className="settings-prose">
        <p>
          When ON, Quill installs context-routing hooks, the <code>quill_*</code> MCP
          tools (index_context, search_context, execute, fetch_and_index, etc.),
          continuity capture, and context-savings telemetry into every enabled provider.
        </p>
        <p>
          Hooks block raw WebFetch and noisy <code>curl</code>/<code>wget</code> dumps and route Bash, Read, Grep, build,
          and test output through the MCP. Large outputs become <code>source:N</code> and{" "}
          <code>chunk:N</code> refs so the assistant stays compact and pulls exact details only on
          demand.
        </p>
        <p>
          Disabling redeploys the base provider integration without context hooks. Historical
          context savings stay searchable in Analytics → Context.
        </p>
        <p>The setting applies globally across Claude Code and Codex.</p>
      </div>

      <div className="settings-section-header">Brevity profile</div>
      <SettingRow
        label="Brevity profile"
        description={
          hasEnabledProvider
            ? "Tells the assistant to compress prose responses (drop articles, hedging, filler) while preserving code, paths, URLs, commands, and other structural content verbatim."
            : "Enable Claude Code or Codex first. The brevity block is written into the enabled provider's managed agent file."
        }
        control={
          <Toggle
            tone={
              features.saving
                ? "busy"
                : !hasEnabledProvider
                  ? "na"
                  : brevityEnabled
                    ? "on"
                    : "off"
            }
            label={
              features.saving
                ? "..."
                : !hasEnabledProvider
                  ? "—"
                  : brevityEnabled
                    ? "ON"
                    : "OFF"
            }
            pressed={brevityEnabled}
            disabled={features.saving || features.loading || !hasEnabledProvider}
            onClick={() => {
              void features
                .setBrevity(!brevityEnabled)
                .catch((e) => toast("error", String(e)));
            }}
          />
        }
      />
      <div className="settings-prose">
        <p>
          Single global toggle. When ON, Quill writes a managed{" "}
          <code>&lt;!-- quill-managed:brevity --&gt;</code> block into{" "}
          <code>~/.claude/CLAUDE.md</code> and <code>~/.codex/AGENTS.md</code> for whichever
          providers are enabled. Newly-enabled providers inherit the current setting; disabling a
          provider strips its block.
        </p>
        <p>
          Disabling globally strips only the managed block; the rest of each file stays intact.
          When <code>AGENTS.md</code> is a symlink to <code>CLAUDE.md</code>, the writer
          canonicalizes the path so a single underlying file is never edited twice. MiniMax has
          no managed agent file and is excluded.
        </p>
      </div>
    </div>
  );
}

export default ContextTab;
