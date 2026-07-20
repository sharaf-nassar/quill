// Shared presentation helpers for the Models systems page. Provider identity
// colors follow the Glass Cockpit provider-family code (DESIGN.md): Claude =
// orange family, Codex = blue family, other providers = violet family,
// sub-agents orchid. A model is a shade of its provider's family ramp,
// assigned by in-scope rank; the same shade renders for that model on every
// surface of the page.

import type { ReactNode } from "react";
import type { ModelIdentity } from "../../../types";
import { modelIdentityKey } from "../../../types";

export const COUNT_FORMATTER = new Intl.NumberFormat("en-US");
export const PERCENT_FORMATTER = new Intl.NumberFormat("en-US", {
	maximumFractionDigits: 1,
});

const DATE_TIME_FORMATTER = new Intl.DateTimeFormat(undefined, {
	year: "numeric",
	month: "short",
	day: "2-digit",
	hour: "2-digit",
	minute: "2-digit",
	timeZoneName: "short",
});

const CLOCK_FORMATTER = new Intl.DateTimeFormat(undefined, {
	hour: "2-digit",
	minute: "2-digit",
});

const SINCE_DATE_FORMATTER = new Intl.DateTimeFormat(undefined, {
	month: "short",
	day: "numeric",
	hour: "2-digit",
	minute: "2-digit",
});

export function providerLabel(provider: string): string {
	if (provider === "claude") return "Claude";
	if (provider === "codex") return "Codex";
	if (provider === "mini_max") return "MiniMax";
	return provider;
}

export function providerBadgeClass(provider: string): string {
	if (provider === "claude") return "model-provider-badge--claude";
	if (provider === "codex") return "model-provider-badge--codex";
	if (provider === "mini_max") return "model-provider-badge--mini-max";
	return "model-provider-badge--other";
}

/**
 * Per-provider model shade ramps, rank 1..6 within provider by delivered
 * order. Rank 7+ folds to the neutral shade. Claude orange is deliberately
 * redder than caution amber (#fbbf24) — amber remains severity-only.
 */
const CLAUDE_SHADES = [
	"#fb923c",
	"#cf4a0c",
	"#fed7aa",
	"#9a3412",
	"#ffedd5",
	"#7c2d12",
] as const;

const CODEX_SHADES = [
	"#60a5fa",
	"#2563eb",
	"#93c5fd",
	"#1d4ed8",
	"#a7cdfd",
	"#16308f",
] as const;

/** Other providers (MiniMax et al.) draw from the violet family. */
const VIOLET_SHADES = [
	"#a78bfa",
	"#7c3aed",
	"#ddd6fe",
	"#5b21b6",
	"#ede9fe",
	"#4c1d95",
] as const;

/** Rank 7+ and unknown identities render neutral, never a generated hue. */
export const NEUTRAL_MODEL_SHADE = "#8b949e";

/** Chart remainder series ("other (N models)") — deliberate flat neutral. */
export const OTHER_SERIES_COLOR = "rgba(230, 237, 243, 0.13)";

function providerShadeRamp(provider: string): readonly string[] {
	if (provider === "claude") return CLAUDE_SHADES;
	if (provider === "codex") return CODEX_SHADES;
	return VIOLET_SHADES;
}

export type ModelShadeMap = ReadonlyMap<string, string>;

/**
 * Scope-stable shade assignment: computed once per overview response from the
 * delivered model order, ranking each model within its provider. Every
 * section (swatches, bars, chart, matrix, pairs, delegation, detail header)
 * must use this same map so a model's color never shifts within the page.
 */
export function buildModelShadeMap(
	models: readonly { identity: ModelIdentity }[],
): ModelShadeMap {
	const shades = new Map<string, string>();
	const rankWithinProvider = new Map<string, number>();
	for (const { identity } of models) {
		const key = modelIdentityKey(identity);
		if (shades.has(key)) continue;
		const rank = rankWithinProvider.get(identity.provider) ?? 0;
		rankWithinProvider.set(identity.provider, rank + 1);
		const ramp = providerShadeRamp(identity.provider);
		shades.set(key, rank < ramp.length ? ramp[rank] : NEUTRAL_MODEL_SHADE);
	}
	return shades;
}

export function modelShade(
	shadeMap: ModelShadeMap,
	identity: ModelIdentity,
): string {
	return shadeMap.get(modelIdentityKey(identity)) ?? NEUTRAL_MODEL_SHADE;
}

/** `#rrggbb` → `rgba(...)` at the given alpha (used for tints and fills). */
export function hexAlpha(hex: string, alpha: number): string {
	const r = Number.parseInt(hex.slice(1, 3), 16);
	const g = Number.parseInt(hex.slice(3, 5), 16);
	const b = Number.parseInt(hex.slice(5, 7), 16);
	if (!Number.isFinite(r) || !Number.isFinite(g) || !Number.isFinite(b)) {
		return hex;
	}
	return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

/**
 * Compact model id for tight columns (matrix headers): drops the redundant
 * "claude-" prefix and trailing -YYYYMMDD date stamps. Full ids stay in
 * title attributes.
 */
export function shortModelId(modelId: string): string {
	return modelId.replace(/-\d{8}$/, "").replace(/^claude-/, "");
}

export function formatDateTime(value: string): string {
	const timestamp = new Date(value);
	return Number.isFinite(timestamp.getTime())
		? DATE_TIME_FORMATTER.format(timestamp)
		: value;
}

/** HH:MM clock label for segment windows and sub-daily chart buckets. */
export function formatClockTime(value: string): string {
	const timestamp = new Date(value);
	return Number.isFinite(timestamp.getTime())
		? CLOCK_FORMATTER.format(timestamp)
		: value;
}

/** "04:49 today" for same-day timestamps, otherwise "Jul 18, 22:14". */
export function formatSince(value: string): string {
	const timestamp = new Date(value);
	if (!Number.isFinite(timestamp.getTime())) return value;
	const nowDate = new Date();
	const sameDay =
		timestamp.getFullYear() === nowDate.getFullYear() &&
		timestamp.getMonth() === nowDate.getMonth() &&
		timestamp.getDate() === nowDate.getDate();
	return sameDay
		? `${CLOCK_FORMATTER.format(timestamp)} today`
		: SINCE_DATE_FORMATTER.format(timestamp);
}

/**
 * Compact relative time for full ISO timestamps (which already carry their
 * offset — unlike utils/time.timeAgo, which appends a "Z").
 */
export function relativeTime(value: string): string {
	const timestamp = new Date(value).getTime();
	if (!Number.isFinite(timestamp)) return value;
	const diffMs = Date.now() - timestamp;
	if (diffMs < 60_000) return "now";
	const mins = Math.floor(diffMs / 60_000);
	if (mins < 60) return `${mins}m ago`;
	const hours = Math.floor(mins / 60);
	if (hours < 24) return `${hours}h ago`;
	const days = Math.floor(hours / 24);
	if (days < 30) return `${days}d ago`;
	const months = Math.floor(days / 30);
	if (months < 12) return `${months}mo ago`;
	return `${Math.floor(days / 365)}y ago`;
}

export interface ProviderBadgeProps {
	provider: string;
	id?: string;
}

export function ProviderBadge({ provider, id }: ProviderBadgeProps) {
	return (
		<span
			id={id}
			className={`model-provider-badge ${providerBadgeClass(provider)}`}
			translate="no"
		>
			{providerLabel(provider)}
		</span>
	);
}

export interface ModelSwatchProps {
	color: string;
}

/** 8px identity swatch — always rides together with the model's name. */
export function ModelSwatch({ color }: ModelSwatchProps) {
	return (
		<span
			className="model-swatch"
			style={{ background: color }}
			aria-hidden="true"
		/>
	);
}

/** Canonical provider grouping for chart stacks and matrix columns. */
export function providerGroupRank(provider: string): number {
	if (provider === "claude") return 0;
	if (provider === "codex") return 1;
	return 2;
}

export interface ModelSectionProps {
	label: string;
	meta?: ReactNode;
	className?: string;
	children: ReactNode;
}

/** Shared hairline-topped section shell with label + right-aligned meta. */
export function ModelSection({
	label,
	meta,
	className,
	children,
}: ModelSectionProps) {
	return (
		<section
			className={className ? `model-section ${className}` : "model-section"}
			aria-label={label}
		>
			<div className="model-section__head">
				<h2 className="model-section__title">{label}</h2>
				{meta !== undefined ? (
					<span className="model-section__meta">{meta}</span>
				) : null}
			</div>
			{children}
		</section>
	);
}
