import { useState, useCallback, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import SearchBar from "../components/sessions/SearchBar";
import FilterBar from "../components/sessions/FilterBar";
import ResultCard from "../components/sessions/ResultCard";
import type {
  SearchFilters,
  SearchResults,
  SearchHit,
  SearchFacets,
  SessionContext,
} from "../types";
import "../styles/sessions.css";

const PAGE_SIZE = 20;

function SessionsWindowView() {
  const [results, setResults] = useState<SearchHit[]>([]);
  const [totalHits, setTotalHits] = useState(0);
  const [queryTimeMs, setQueryTimeMs] = useState(0);
  const [facets, setFacets] = useState<SearchFacets>({
    projects: [],
    hosts: [],
  });
  const [filters, setFilters] = useState<SearchFilters>({});
  const [expandedHit, setExpandedHit] = useState<string | null>(null);
  const [context, setContext] = useState<Record<string, SessionContext>>({});
  const [loading, setLoading] = useState(false);
  const [page, setPage] = useState(0);
  const [query, setQuery] = useState("");

  useEffect(() => {
    invoke<SearchFacets>("get_search_facets").then(setFacets).catch(() => {});
  }, []);

  const handleSearch = useCallback(
    async (value: string) => {
      setQuery(value);
      setPage(0);
      setExpandedHit(null);
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
    [filters],
  );

  const handleLoadMore = useCallback(async () => {
    const nextPage = page + 1;
    setLoading(true);
    try {
      const res = await invoke<SearchResults>("search_sessions", {
        query,
        filters,
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
  }, [query, filters, page]);

  const handleExpand = useCallback(
    async (hit: SearchHit) => {
      if (expandedHit === hit.message_id) {
        setExpandedHit(null);
        return;
      }
      setExpandedHit(hit.message_id);
      if (!context[hit.message_id]) {
        try {
          const ctx = await invoke<SessionContext>("get_session_context", {
            sessionId: hit.session_id,
            aroundMessageId: hit.message_id,
            window: 5,
          });
          setContext((prev) => ({ ...prev, [hit.message_id]: ctx }));
        } catch {
          /* no-op */
        }
      }
    },
    [expandedHit, context],
  );

  const handleFiltersChange = useCallback(
    (newFilters: SearchFilters) => {
      setFilters(newFilters);
      if (query.trim()) {
        setPage(0);
        setExpandedHit(null);
        setLoading(true);
        invoke<SearchResults>("search_sessions", {
          query,
          filters: newFilters,
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
    [query],
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
      <div className="sessions-window-body">
        <SearchBar onSearch={handleSearch} />
        <FilterBar
          facets={facets}
          filters={filters}
          onChange={handleFiltersChange}
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
            key={hit.message_id}
            hit={hit}
            expanded={expandedHit === hit.message_id}
            context={context[hit.message_id] ?? null}
            onToggle={() => handleExpand(hit)}
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
  );
}

export default SessionsWindowView;
