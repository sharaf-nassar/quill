import type {
	ModelAnalyticsError,
	ModelAnalyticsErrorCode,
} from "../types";

const MODEL_ANALYTICS_ERROR_CODES = new Set<ModelAnalyticsErrorCode>([
	"invalid_range",
	"invalid_provider",
	"invalid_model_id",
	"invalid_cursor",
	"not_found",
	"storage_error",
]);

const DEFAULT_ERROR_MESSAGE =
	"Model analytics could not be loaded. Retry this section.";

function parseErrorCandidate(value: unknown): unknown {
	if (value instanceof Error) {
		return parseErrorCandidate(value.message);
	}

	if (typeof value !== "string") return value;

	try {
		return JSON.parse(value) as unknown;
	} catch {
		return value;
	}
}

/**
 * Preserve the shared model-analytics IPC envelope while keeping unexpected
 * failures bounded and safe for display. Tauri may reject with either the
 * serialized object or a JSON string depending on the runtime boundary.
 */
export function normalizeModelAnalyticsError(
	error: unknown,
	fallbackMessage = DEFAULT_ERROR_MESSAGE,
): ModelAnalyticsError {
	const candidate = parseErrorCandidate(error);
	if (candidate && typeof candidate === "object") {
		const code = Reflect.get(candidate, "code");
		const message = Reflect.get(candidate, "message");
		if (
			typeof code === "string" &&
			MODEL_ANALYTICS_ERROR_CODES.has(code as ModelAnalyticsErrorCode) &&
			typeof message === "string"
		) {
			return { code: code as ModelAnalyticsErrorCode, message };
		}
	}

	return {
		code: "storage_error",
		message: fallbackMessage,
	};
}
