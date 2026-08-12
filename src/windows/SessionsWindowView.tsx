import { useState, useCallback, useEffect, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import SearchBar from "../components/sessions/SearchBar";
import FilterBar from "../components/sessions/FilterBar";
import ResultCard from "../components/sessions/ResultCard";
import DetailPanel from "../components/sessions/DetailPanel";
import { useSessionCodeStats } from "../hooks/useSessionCodeStats";
import { useRetentionCutoff } from "../hooks/useRetentionCutoff";
import { RetentionBanner } from "../components/RetentionBanner";
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
	// Feature 014: `get_batch_session_code_stats` is one of the three readers
	// retention can starve. The index behind the search itself is never pruned,
	// so results keep appearing after their line counts are gone.
	const { cutoff: retentionCutoff } = useRetentionCutoff();
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
	const [syncingIndex, setSyncingIndex] = useState(true);
	const [syncError, setSyncError] = useState<string | null>(null);
	const [loading, setLoading] = useState(false);
	const [page, setPage] = useState(0);
	const [query, setQuery] = useState("");
	const searchRequestRef = useRef(0);
	const hitKey = useCallback(
		(hit: Pick<SearchHit, "provider" | "message_id">) =>
			`${hit.provider}:${hit.message_id}`,
		[],
	);

	useEffect(() => {
		let cancelled = false;

		const syncIndex = async () => {
			setSyncingIndex(true);
			setSyncError(null);

			try {
				await invoke<number>("sync_search_index");
			} catch (error) {
				if (!cancelled) {
					setSyncError(String(error));
				}
			}

			try {
				const nextFacets = await invoke<SearchFacets>("get_search_facets");
				if (!cancelled) {
					setFacets(nextFacets);
				}
			} catch (error) {
				if (!cancelled) {
					setSyncError((prev) => prev ?? String(error));
				}
			} finally {
				if (!cancelled) {
					setSyncingIndex(false);
				}
			}
		};

		void syncIndex();

		return () => {
			cancelled = true;
		};
	}, []);

	const runSearch = useCallback(
		async (
			nextQuery: string,
			nextFilters: SearchFilters,
			nextSort: SortMode,
			nextPage: number,
		) => {
			const requestId = ++searchRequestRef.current;
			if (!nextQuery.trim()) {
				setResults([]);
				setTotalHits(0);
				setQueryTimeMs(0);
				setLoading(false);
				return;
			}

			setLoading(true);
			try {
				const res = await invoke<SearchResults>("search_sessions", {
					query: nextQuery,
					filters: nextFilters,
					sortBy: nextSort,
					page: nextPage,
					pageSize: PAGE_SIZE,
				});
				if (searchRequestRef.current !== requestId) return;
				setResults((previous) =>
					nextPage === 0 ? res.hits : [...previous, ...res.hits],
				);
				setTotalHits(res.total_hits);
				setQueryTimeMs(res.query_time_ms);
				setPage(nextPage);
			} catch {
				if (searchRequestRef.current === requestId && nextPage === 0) {
					setResults([]);
					setTotalHits(0);
				}
			} finally {
				if (searchRequestRef.current === requestId) setLoading(false);
			}
		},
		[],
	);

	useEffect(
		() => () => {
			searchRequestRef.current += 1;
		},
		[],
	);

	const handleSearch = useCallback(
		(value: string) => {
			setQuery(value);
			setPage(0);
			setSelectedHit(null);
			void runSearch(value, filters, sortBy, 0);
		},
		[filters, runSearch, sortBy],
	);

	const handleLoadMore = useCallback(() => {
		const nextPage = page + 1;
		void runSearch(query, filters, sortBy, nextPage);
	}, [filters, page, query, runSearch, sortBy]);

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
				void runSearch(query, newFilters, sortBy, 0);
			}
		},
		[query, runSearch, sortBy],
	);

	const handleSortChange = useCallback(
		(newSort: SortMode) => {
			setSortBy(newSort);
			if (query.trim()) {
				setPage(0);
				setSelectedHit(null);
				void runSearch(query, filters, newSort, 0);
			}
		},
		[filters, query, runSearch],
	);

	return (
		<div className="sessions-window">
			<div className="sessions-split">
				<div className="sessions-list-panel">
					<div className="sessions-list-scroll">
						{syncingIndex ? (
							<div className="sessions-loading">Refreshing session index...</div>
						) : (
							<>
								<SearchBar onSearch={handleSearch} />
								<FilterBar
									facets={facets}
									filters={filters}
									onChange={handleFiltersChange}
									sortBy={sortBy}
									onSortChange={handleSortChange}
								/>
								{syncError && (
									<div className="sessions-loading">
										Session index refresh failed. Results may be stale.
									</div>
								)}
								<RetentionBanner cutoff={retentionCutoff} />
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
										retentionCutoff={retentionCutoff}
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
							</>
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
							retentionCutoff={retentionCutoff}
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
