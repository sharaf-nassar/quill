import { openUrl } from "@tauri-apps/plugin-opener";

// A bare <a target="_blank"> does nothing inside a Tauri v2 webview: nothing
// handles the "create new window" request, so the click is silently dropped.
// Every external link therefore routes through the opener plugin, whose
// capability scope (`opener:allow-default-urls`) is what actually restricts
// which schemes may leave the app.
export async function openExternal(url: string): Promise<void> {
  try {
    await openUrl(url);
  } catch (error) {
    // Refusals are the scope doing its job, so report rather than retry.
    console.error(`Could not open ${url} externally:`, error);
  }
}

/** Click handler for anchors that keeps href for hover, focus, and copy-link. */
export function handleExternalClick(
  event: React.MouseEvent<HTMLAnchorElement>,
): void {
  event.preventDefault();
  void openExternal(event.currentTarget.href);
}
