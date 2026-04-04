import type { IntegrationProvider, ProviderFilter } from "../types";

const PROVIDER_ORDER: IntegrationProvider[] = ["claude", "codex", "mini_max"];

export function providerLabel(provider: IntegrationProvider): string {
  if (provider === "claude") return "Claude";
  if (provider === "codex") return "Codex";
  return "MiniMax";
}

export function normalizeProviderScope(
  scope: IntegrationProvider[] | null | undefined,
): IntegrationProvider[] {
  const raw: IntegrationProvider[] =
    scope && scope.length > 0 ? scope : ["claude"];
  return [...new Set(raw)].sort(
    (left, right) => PROVIDER_ORDER.indexOf(left) - PROVIDER_ORDER.indexOf(right),
  );
}

export function providerScopeLabel(
  scope: IntegrationProvider[] | null | undefined,
): string {
  const normalized = normalizeProviderScope(scope);
  if (normalized.length > 1) {
    return "Shared";
  }
  return providerLabel(normalized[0]);
}

export function providerFilterLabel(filter: ProviderFilter): string {
  return filter === "all" ? "All Providers" : providerLabel(filter);
}

export function providerBadgeClass(provider: IntegrationProvider): string {
  return `learning-provider-badge learning-provider-badge--${provider}`;
}

export function providerScopeClass(
  scope: IntegrationProvider[] | null | undefined,
): string {
  const normalized = normalizeProviderScope(scope);
  if (normalized.length > 1) {
    return "learning-provider-badge learning-provider-badge--shared";
  }
  return providerBadgeClass(normalized[0]);
}

export function memoryTypeLabel(memoryType: string | null | undefined): string {
  switch (memoryType) {
    case "claude-md":
      return "CLAUDE.md";
    case "agents-md":
      return "AGENTS.md";
    case "user":
      return "user";
    case "feedback":
      return "feedback";
    case "project":
      return "project";
    case "reference":
      return "reference";
    default:
      return memoryType ?? "memory";
  }
}
