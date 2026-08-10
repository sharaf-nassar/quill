import type { RangeType } from "../../types";

/** Widget scopes exclude the full-page 30-day range. */
export const WIDGET_RANGES = [
  { id: "1h", label: "1H" },
  { id: "6h", label: "6H" },
  { id: "24h", label: "24H" },
  { id: "7d", label: "7D" },
] as const satisfies ReadonlyArray<{ id: RangeType; label: string }>;

export const DEFAULT_WIDGET_RANGE: RangeType = "1h";
const STORAGE_KEY = "quill-widget-range";

export function resolveStoredWidgetRange(value: string | null): RangeType {
  return WIDGET_RANGES.some((range) => range.id === value)
    ? (value as RangeType)
    : DEFAULT_WIDGET_RANGE;
}

export function readStoredWidgetRange(
  storage?: Pick<Storage, "getItem">,
): RangeType {
  try {
    return resolveStoredWidgetRange((storage ?? localStorage).getItem(STORAGE_KEY));
  } catch {
    return DEFAULT_WIDGET_RANGE;
  }
}

export function storeWidgetRange(
  range: RangeType,
  storage?: Pick<Storage, "setItem">,
): void {
  try {
    (storage ?? localStorage).setItem(STORAGE_KEY, range);
  } catch {
    /* Keep the current-session selection when storage is unavailable. */
  }
}
