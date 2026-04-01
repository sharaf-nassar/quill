import type { InstalledPlugin, PluginUpdate } from "../types";

function normalizeProjectPath(projectPath: string | null | undefined): string {
	return projectPath ?? "";
}

export function installedPluginInstanceKey(
	plugin: Pick<InstalledPlugin, "provider" | "plugin_id" | "scope" | "project_path">,
): string {
	return [
		plugin.provider,
		plugin.plugin_id,
		plugin.scope,
		normalizeProjectPath(plugin.project_path),
	].join(":");
}

export function pluginUpdateInstanceKey(
	update: Pick<
		PluginUpdate,
		"provider" | "name" | "marketplace" | "scope" | "project_path"
	>,
): string {
	return [
		update.provider,
		`${update.name}@${update.marketplace}`,
		update.scope,
		normalizeProjectPath(update.project_path),
	].join(":");
}
