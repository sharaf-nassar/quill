//! Default-deny command boundary for browser invokes.
//!
//! This module is the single Rust source of truth for which Tauri commands the
//! web listener may dispatch; the normative table is
//! `specs/029-web-ui-server.md#exact-permitted-command-table`. The match is on
//! the complete invoke command string, so no prefix, suffix, alias, or
//! `plugin:*` name can widen it, and nothing is keyed on a command count.
//!
//! Every permitted arm resolves to a SQLite read through
//! [[src-tauri/src/storage.rs]] or to the in-process usage cache the desktop
//! already produced. None of them contacts a provider API, mutates settings,
//! or schedules maintenance work, so a browser viewer cannot spend the user's
//! quota or change desktop state.

/// Whether the web listener may dispatch `command`. Default-deny.
pub fn is_permitted_command(command: &str) -> bool {
    match command {
        // `Storage::get_activity_series` — bucketed counts over `usage_snapshots`.
        "get_activity_series" => true,
        // `get_cached_usage_data` — the desktop's last usage snapshot; the
        // deliberately network-free replacement for `fetch_usage_data`.
        "get_cached_usage_data" => true,
        // `Storage::get_code_stats` — range totals over `tool_actions`.
        "get_code_stats" => true,
        // `Storage::get_code_stats_history` — bucketed `tool_actions` history.
        "get_code_stats_history" => true,
        // `Storage::get_context_savings_analytics` — context-savings event reads.
        "get_context_savings_analytics" => true,
        // `integrations::cpa::connection_status` — two `settings` reads.
        "get_cpa_connection_status" => true,
        // `Storage::get_hook_breakdown` — aggregates `hook_invocations`.
        "get_hook_breakdown" => true,
        // `Storage::get_host_breakdown` — aggregates `token_usage` by hostname.
        "get_host_breakdown" => true,
        // `Storage::get_llm_runtime_stats` — reads `session_events`/`runtime_hourly`.
        "get_llm_runtime_stats" => true,
        // `Storage::get_model_usage_overview` — reads model observations and rollups.
        "get_model_usage_overview" => true,
        // `Storage::get_project_breakdown` — aggregates `token_usage` by cwd.
        "get_project_breakdown" => true,
        // `integrations::load_statuses` — saved provider rows, no filesystem rescan.
        "get_provider_statuses" => true,
        // `Storage::get_retention_policy` — one `settings` read.
        "get_retention_policy" => true,
        // `get_session_breakdown` — retained rows plus the live tracker's
        // already-folded in-memory map; the overlay writes nothing.
        "get_session_breakdown" => true,
        // `Storage::get_skill_breakdown` — aggregates `skill_usages`.
        "get_skill_breakdown" => true,
        // `Storage::get_token_history` — reads `token_usage` points.
        "get_token_history" => true,
        // `Storage::get_widget_activity_stats` — tool-call, prompt, and
        // reasoning counts over `tool_actions`, `session_events`, and model
        // observations.
        "get_widget_activity_stats" => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Browser command default-deny]]
    #[test]
    fn command_boundary_is_default_deny() {
        for command in [
            "get_activity_series",
            "get_cached_usage_data",
            "get_code_stats",
            "get_code_stats_history",
            "get_context_savings_analytics",
            "get_cpa_connection_status",
            "get_hook_breakdown",
            "get_host_breakdown",
            "get_llm_runtime_stats",
            "get_model_usage_overview",
            "get_project_breakdown",
            "get_provider_statuses",
            "get_retention_policy",
            "get_session_breakdown",
            "get_skill_breakdown",
            "get_token_history",
            "get_widget_activity_stats",
        ] {
            assert!(is_permitted_command(command), "permitted: {command}");
        }

        // Provider-spending reads, retry/backfill work, and maintenance.
        for command in [
            "fetch_usage_data",
            "refresh_usage_data",
            "retry_model_history_backfill",
            "rescan_integrations",
            "rebuild_model_rollup",
            "compact_database",
            "run_retention_maintenance",
            "preview_retention",
            "trigger_analysis",
            "trigger_memory_optimization",
            "install_app_update",
            "sync_search_index",
            "hide_window",
            "quit_app",
        ] {
            assert!(!is_permitted_command(command), "denied: {command}");
        }

        // Every registered setter and every other state mutation.
        for command in [
            "set_activity_tracking_enabled",
            "set_brevity_enabled",
            "set_context_preservation_enabled",
            "set_context_telemetry_enabled",
            "set_cpa_connection",
            "set_indicator_primary_provider",
            "set_learning_settings",
            "set_minimax_api_key",
            "set_retention_policy",
            "set_runtime_settings",
            "set_web_ui_config",
            "clear_cpa_connection",
            "confirm_enable_provider",
            "confirm_disable_provider",
            "regenerate_web_pairing_code",
            "add_custom_project",
            "remove_custom_project",
            "delete_learned_rule",
            "delete_memory_file",
            "delete_project_memories",
            "promote_learned_rule",
            "submit_rule_feedback",
            "approve_suggestion",
            "approve_suggestion_group",
            "deny_suggestion",
            "deny_suggestion_group",
            "undeny_suggestion",
            "undo_suggestion",
            "integrate_appimage",
        ] {
            assert!(!is_permitted_command(command), "denied setter: {command}");
        }

        // The whole `plugin:*` namespace, including the event seam the browser
        // no-ops client-side.
        for namespace in [
            "event", "window", "webview", "updater", "dialog", "log", "path",
        ] {
            for command in ["listen", "unlisten", "emit", "set_zoom", "open"] {
                let name = format!("plugin:{namespace}|{command}");
                assert!(!is_permitted_command(&name), "denied plugin: {name}");
            }
        }

        // Unknown names, and near-misses that a prefix or suffix match would admit.
        for command in [
            "",
            "unknown_command",
            "get_token_history_extra",
            "get_token_history ",
            " get_token_history",
            "GET_TOKEN_HISTORY",
            "get_token",
            "plugin:event|get_token_history",
        ] {
            assert!(!is_permitted_command(command), "denied unknown: {command}");
        }
    }
}
