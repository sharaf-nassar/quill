import { useState, useCallback, useEffect, useMemo } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import SearchBar from "../components/sessions/SearchBar";
import FilterBar from "../components/sessions/FilterBar";
import ResultCard from "../components/sessions/ResultCard";
import DetailPanel from "../components/sessions/DetailPanel";
import { useSessionCodeStats } from "../hooks/useSessionCodeStats";
import type {
	SearchFilters,
	SearchResults,
	SearchHit,
	SearchFacets,
	SessionContext,
	SessionRef,
	SortMode,
} from "../types";
import { sessionRefKey } from "../types";
import "../styles/sessions.css";

const PAGE_SIZE = 20;

function SessionsWindowView() {
	const [results, setResults] = useState<SearchHit[]>([]);
	const sessionRefs = useMemo(() => {
		const seen = new Set<string>();
		return results.reduce<SessionRef[]>((refs, hit) => {
			const ref = { provider: hit.provider, session_id: hit.session_id };
			const key = sessionRefKey(ref);
			if (!seen.has(key)) {
				seen.add(key);
				refs.push(ref);
			}
			return refs;
		}, []);
	}, [results]);
	const locStatsMap = useSessionCodeStats(sessionRefs);
	const [totalHits, setTotalHits] = useState(0);
	const [queryTimeMs, setQueryTimeMs] = useState(0);
	const [facets, setFacets] = useState<SearchFacets>({
		providers: [],
		projects: [],
		hosts: [],
	});
	const [filters, setFilters] = useState<SearchFilters>({});
	const [sortBy, setSortBy] = useState<SortMode>("relevance");
	const [selectedHit, setSelectedHit] = useState<SearchHit | null>(null);
	const [context, setContext] = useState<Record<string, SessionContext>>({});
	const [loading, setLoading] = useState(false);
	const [page, setPage] = useState(0);
	const [query, setQuery] = useState("");
	const hitKey = useCallback(
		(hit: Pick<SearchHit, "provider" | "message_id">) =>
			`${hit.provider}:${hit.message_id}`,
		[],
	);

	useEffect(() => {
		invoke<SearchFacets>("get_search_facets").then(setFacets).catch(() => {});
	}, []);

	const handleSearch = useCallback(
		async (value: string) => {
			setQuery(value);
			setPage(0);
			setSelectedHit(null);
			if (!value.trim()) {
				setResults([]);
				setTotalHits(0);
				setQueryTimeMs(0);
				return;
			}
			setLoading(true);
			try {
				const res = await invoke<SearchResults>("search_sessions", {
					query: value,
					filters,
					sortBy,
					page: 0,
					pageSize: PAGE_SIZE,
				});
				setResults(res.hits);
				setTotalHits(res.total_hits);
				setQueryTimeMs(res.query_time_ms);
			} catch {
				setResults([]);
				setTotalHits(0);
			} finally {
				setLoading(false);
			}
		},
		[filters, sortBy],
	);

	const handleLoadMore = useCallback(async () => {
		const nextPage = page + 1;
		setLoading(true);
		try {
			const res = await invoke<SearchResults>("search_sessions", {
				query,
				filters,
				sortBy,
				page: nextPage,
				pageSize: PAGE_SIZE,
			});
			setResults((prev) => [...prev, ...res.hits]);
			setPage(nextPage);
		} catch {
			/* no-op */
		} finally {
			setLoading(false);
		}
	}, [query, filters, sortBy, page]);

	const handleSelect = useCallback(
		async (hit: SearchHit) => {
			setSelectedHit(hit);
			const key = hitKey(hit);
			if (!context[key]) {
				try {
					const ctx = await invoke<SessionContext>("get_session_context", {
						provider: hit.provider,
						sessionId: hit.session_id,
						aroundMessageId: hit.message_id,
						window: 5,
					});
					setContext((prev) => ({ ...prev, [key]: ctx }));
				} catch {
					/* no-op */
				}
			}
		},
		[context, hitKey],
	);

	const handleFiltersChange = useCallback(
		(newFilters: SearchFilters) => {
			setFilters(newFilters);
			if (query.trim()) {
				setPage(0);
				setSelectedHit(null);
				setLoading(true);
				invoke<SearchResults>("search_sessions", {
					query,
					filters: newFilters,
					sortBy,
					page: 0,
					pageSize: PAGE_SIZE,
				})
					.then((res) => {
						setResults(res.hits);
						setTotalHits(res.total_hits);
						setQueryTimeMs(res.query_time_ms);
					})
					.catch(() => {
						setResults([]);
						setTotalHits(0);
					})
					.finally(() => setLoading(false));
			}
		},
		[query, sortBy],
	);

	const handleSortChange = useCallback(
		(newSort: SortMode) => {
			setSortBy(newSort);
			if (query.trim()) {
				setPage(0);
				setSelectedHit(null);
				setLoading(true);
				invoke<SearchResults>("search_sessions", {
					query,
					filters,
					sortBy: newSort,
					page: 0,
					pageSize: PAGE_SIZE,
				})
					.then((res) => {
						setResults(res.hits);
						setTotalHits(res.total_hits);
						setQueryTimeMs(res.query_time_ms);
					})
					.catch(() => {
						setResults([]);
						setTotalHits(0);
					})
					.finally(() => setLoading(false));
			}
		},
		[query, filters],
	);

	const handleClose = async () => {
		await getCurrentWindow().close();
	};

	return (
		<div className="sessions-window">
			<div className="sessions-window-titlebar" data-tauri-drag-region>
				<span className="sessions-window-title" data-tauri-drag-region>
					Session Search
				</span>
				<button
					className="sessions-window-close"
					onClick={handleClose}
					aria-label="Close"
				>
					&times;
				</button>
			</div>
			<div className="sessions-split">
				<div className="sessions-list-panel">
					<div className="sessions-list-scroll">
						<SearchBar onSearch={handleSearch} />
						<FilterBar
							facets={facets}
							filters={filters}
							onChange={handleFiltersChange}
							sortBy={sortBy}
							onSortChange={handleSortChange}
						/>
						{query.trim() && !loading && (
							<div className="sessions-results-header">
								{totalHits} result{totalHits !== 1 ? "s" : ""} in {queryTimeMs}ms
							</div>
						)}
						{loading && results.length === 0 && (
							<div className="sessions-loading">Searching...</div>
						)}
						{results.map((hit) => (
							<ResultCard
								key={hitKey(hit)}
								hit={hit}
								selected={
									selectedHit ? hitKey(selectedHit) === hitKey(hit) : false
								}
								locStats={
									locStatsMap[sessionRefKey({
										provider: hit.provider,
										session_id: hit.session_id,
									})] ?? null
								}
								onSelect={() => handleSelect(hit)}
							/>
						))}
						{results.length >= PAGE_SIZE &&
							results.length < totalHits && (
								<button
									className="sessions-load-more"
									onClick={handleLoadMore}
									disabled={loading}
								>
									{loading ? "Loading..." : "Load more"}
								</button>
							)}
					</div>
				</div>
				<div className="sessions-detail-panel">
					{selectedHit ? (
						<DetailPanel
							hit={selectedHit}
							context={context[hitKey(selectedHit)] ?? null}
							locStats={
								locStatsMap[sessionRefKey({
									provider: selectedHit.provider,
									session_id: selectedHit.session_id,
								})] ?? null
							}
						/>
					) : (
						<div className="sessions-detail-empty">
							Select a result to view details
						</div>
					)}
				</div>
			</div>
		</div>
	);
}

export default SessionsWindowView;
