import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useToast } from "../../hooks/useToast";
import { providerLabel } from "../../utils/providers";
import type {
	IntegrationProvider,
	RestartInstance,
	RestartStatus,
	InstanceStatus,
} from "../../types";

function statusKey(status: InstanceStatus): string {
	if (typeof status === "string") return status.toLowerCase();
	return "failed";
}

function statusText(status: InstanceStatus): string {
	if (typeof status === "string") return status;
	return "Failed";
}

function terminalLabel(inst: RestartInstance): string {
	if (inst.terminal_type.type === "Tmux") {
		return `tmux:${inst.terminal_type.target}`;
	}
	const match = inst.tty.match(/pts\/(\d+)$/);
	return match ? `pts/${match[1]}` : inst.tty;
}

function shortenCwd(cwd: string): string {
	return cwd.replace(/^\/home\/[^/]+/, "~");
}

function formatElapsed(seconds: number): string {
	const m = Math.floor(seconds / 60);
	const s = seconds % 60;
	if (m === 0) return `${s}s`;
	return `${m}m ${s}s`;
}

const PROVIDER_ORDER: IntegrationProvider[] = ["claude", "codex", "mini_max"];

function providerHeading(provider: IntegrationProvider): string {
	if (provider === "claude") return "Claude Code";
	if (provider === "codex") return "Codex";
	return "MiniMax";
}

function providerSetupMessage(provider: IntegrationProvider): string {
	if (provider === "claude") return "Claude restart hooks are not installed.";
	if (provider === "codex") return "Codex restart shell integration is not installed.";
	return "MiniMax restart shell integration is not installed.";
}

function RestartPanel() {
	const { toast } = useToast();
	const [status, setStatus] = useState<RestartStatus | null>(null);
	const [hookStatusByProvider, setHookStatusByProvider] = useState<
		Partial<Record<IntegrationProvider, boolean>>
	>({});
	const [installingProvider, setInstallingProvider] =
		useState<IntegrationProvider | null>(null);
	const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

	const fetchStatus = useCallback(async () => {
		const result = await invoke<RestartStatus>("get_restart_status");
		setStatus(result);

		if (result.phase === "Complete") {
			const allOk = result.instances.every(
				(i) => typeof i.status === "string" && i.status !== "Unknown",
			);
			if (allOk) {
				setTimeout(async () => {
					await getCurrentWindow().close();
				}, 3000);
			}
		}
	}, []);

	useEffect(() => {
		fetchStatus();
		pollRef.current = setInterval(fetchStatus, 1000);
		const unlisten = listen("restart-status-changed", fetchStatus);
		return () => {
			if (pollRef.current) clearInterval(pollRef.current);
			unlisten.then((fn) => fn());
		};
	}, [fetchStatus]);

	const currentPhase = status?.phase;
	const providersWithInstances = useMemo(() => {
		if (!status) {
			return [] as IntegrationProvider[];
		}

		return Array.from(
			new Set(status.instances.map((instance) => instance.provider)),
		).sort(
			(left, right) =>
				PROVIDER_ORDER.indexOf(left) - PROVIDER_ORDER.indexOf(right),
		);
	}, [status]);
	const providersWithInstancesKey = useMemo(
		() => providersWithInstances.join(","),
		[providersWithInstances],
	);
	const groupedInstances = useMemo(() => {
		if (!status) {
			return [] as Array<{
				provider: IntegrationProvider;
				instances: RestartInstance[];
			}>;
		}

		return providersWithInstances.map((provider) => ({
			provider,
			instances: status.instances.filter((instance) => instance.provider === provider),
		}));
	}, [providersWithInstances, status]);

	const readHookStatuses = useCallback(
		async (providers: IntegrationProvider[]) => {
			const entries = await Promise.all(
				providers.map(async (provider) => {
					try {
						const installed = await invoke<boolean>(
							"check_restart_hooks_installed",
							{ provider },
						);
						return [provider, installed] as const;
					} catch {
						return [provider, false] as const;
					}
				}),
			);

			return Object.fromEntries(entries) as Partial<
				Record<IntegrationProvider, boolean>
			>;
		},
		[],
	);

	useEffect(() => {
		const activeProviders = providersWithInstancesKey
			? (providersWithInstancesKey.split(",") as IntegrationProvider[])
			: [];

		if (activeProviders.length === 0) {
			setHookStatusByProvider({});
			return;
		}

		let cancelled = false;
		void (async () => {
			const statuses = await readHookStatuses(activeProviders);
			if (cancelled) {
				return;
			}
			setHookStatusByProvider(statuses);
		})();

		return () => {
			cancelled = true;
		};
	}, [providersWithInstancesKey, readHookStatuses]);

	useEffect(() => {
		if (currentPhase === "Complete") {
			if (pollRef.current) {
				clearInterval(pollRef.current);
				pollRef.current = null;
			}
		}
	}, [currentPhase]);

	const handleRestart = useCallback(async (force: boolean) => {
		await invoke("request_restart", { force });
	}, []);

	const handleCancel = useCallback(async () => {
		await invoke("cancel_restart");
		toast("info", "Restart cancelled.");
		const result = await invoke<RestartStatus>("get_restart_status");
		setStatus(result);
	}, [toast]);

	const handleInstallHooks = useCallback(async (provider: IntegrationProvider) => {
		setInstallingProvider(provider);
		try {
			await invoke("install_restart_hooks", { provider });
			const activeProviders = providersWithInstancesKey
				? (providersWithInstancesKey.split(",") as IntegrationProvider[])
				: [provider];
			setHookStatusByProvider(await readHookStatuses(activeProviders));
			toast("info", `${providerHeading(provider)} restart integration installed.`);
		} catch (e) {
			console.error("Failed to install hooks:", e);
		} finally {
			setInstallingProvider(null);
		}
	}, [providersWithInstancesKey, readHookStatuses, toast]);

	if (!status) {
		return <div className="restart-panel restart-panel--loading">Loading...</div>;
	}

	const { phase, instances, waiting_on, elapsed_seconds } = status;
	const isWaiting = phase === "WaitingForIdle";
	const isRestarting = phase === "Restarting";
	const isComplete = phase === "Complete";
	const isTimedOut = phase === "TimedOut";
	const canRestart = phase === "Idle" || phase === "Cancelled";
	const instanceCount = instances.length;
	const hasCodexInstances = providersWithInstances.includes("codex");
	const providersNeedingSetup = providersWithInstances.filter(
		(provider) => hookStatusByProvider[provider] === false,
	);

	return (
		<div className="restart-panel">
			<div className="restart-list">
				{instanceCount === 0 ? (
					<div className="restart-empty">
						No restartable Claude or Codex sessions found.
					</div>
				) : (
					groupedInstances.map(({ provider, instances: providerInstances }) => (
						<div className="restart-group" key={provider}>
							<div className="restart-group__header">
								<span className={`restart-row__provider restart-row__provider--${provider}`}>
									{providerLabel(provider)}
								</span>
								<span className="restart-group__title">
									{providerHeading(provider)}
								</span>
								<span className="restart-group__count">
									{providerInstances.length} instance
									{providerInstances.length !== 1 ? "s" : ""}
								</span>
							</div>
								{providerInstances.map((inst) => (
									<div className="restart-row" key={`${inst.provider}:${inst.pid}`}>
										<div className="restart-row__info">
											<div className="restart-row__meta">
												<div className="restart-row__cwd" title={inst.cwd}>
													{shortenCwd(inst.cwd)}
												</div>
											</div>
											<div className="restart-row__terminal">
												{terminalLabel(inst)}
											</div>
										</div>
										<span className={`restart-row__status restart-row__status--${statusKey(inst.status)}`}>
											<span className="restart-row__status-dot" />
											{statusText(inst.status)}
										</span>
									</div>
								))}
							</div>
						))
					)}
			</div>

			<div className="restart-footer">
				<span className={`restart-footer__info${
					isWaiting ? " restart-footer__info--waiting" :
					isComplete ? " restart-footer__info--success" :
					isTimedOut ? " restart-footer__info--warning" : ""
				}`}>
					{isWaiting && `Waiting for ${waiting_on}... ${formatElapsed(elapsed_seconds)}`}
					{isRestarting && "Restarting..."}
					{isComplete && "Restart complete"}
					{isTimedOut && "Timed out"}
					{canRestart && `${instanceCount} instance${instanceCount !== 1 ? "s" : ""}`}
				</span>
				<div className="restart-footer__actions">
					{(isWaiting || isTimedOut) && (
						<button
							className="restart-btn restart-btn--secondary"
							onClick={handleCancel}
						>
							Cancel
						</button>
					)}
					{isTimedOut && (
						<button
							className="restart-btn restart-btn--primary"
							onClick={() => handleRestart(true)}
						>
							Force Restart
						</button>
					)}
					{canRestart && (
						<button
							className="restart-btn restart-btn--primary"
							onClick={() => handleRestart(false)}
							disabled={instanceCount === 0}
						>
							Restart All
						</button>
					)}
				</div>
			</div>

			{hasCodexInstances && (
				<div className="restart-provider-note">
					Codex does not expose an idle signal, so Codex restarts proceed
					immediately instead of waiting for quiescence.
				</div>
			)}

			{providersNeedingSetup.map((provider) => (
				<div className="restart-hook-banner" key={provider}>
					<span>{providerSetupMessage(provider)}</span>
					<button
						className="restart-btn restart-btn--primary"
						onClick={() => handleInstallHooks(provider)}
						disabled={installingProvider !== null}
					>
						{installingProvider === provider
							? "Installing..."
							: provider === "claude"
								? "Install Hooks"
								: "Install Integration"}
					</button>
				</div>
			))}
		</div>
	);
}

export default RestartPanel;
