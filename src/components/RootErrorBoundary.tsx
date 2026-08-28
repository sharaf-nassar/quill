import { Component, type ErrorInfo, type ReactNode } from "react";

interface RootErrorBoundaryProps {
  children: ReactNode;
}

interface RootErrorBoundaryState {
  message: string | null;
}

/**
 * The last thing between a render failure and an empty window.
 *
 * React unmounts the whole tree when nothing catches an error, and every Quill
 * window is one lazily-imported view under a `Suspense` that handles pending
 * but not rejected. So a failed chunk — a dev server mid-reoptimize, a
 * half-written build — painted a window with the page background and nothing
 * else: no message, no route, nothing to act on. A blank window is never an
 * acceptable failure, so this reports what broke and offers the reload that
 * fixes the common case.
 */
class RootErrorBoundary extends Component<
  RootErrorBoundaryProps,
  RootErrorBoundaryState
> {
  state: RootErrorBoundaryState = { message: null };

  static getDerivedStateFromError(error: unknown): RootErrorBoundaryState {
    return {
      message: error instanceof Error ? error.message : String(error),
    };
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error("Quill failed to render:", error, info.componentStack);
  }

  render(): ReactNode {
    if (this.state.message === null) return this.props.children;
    return (
      <div className="root-error" role="alert">
        <p className="root-error-title">This window failed to load.</p>
        <p className="root-error-detail">{this.state.message}</p>
        <button
          type="button"
          className="root-error-action"
          onClick={() => window.location.reload()}
        >
          Reload
        </button>
      </div>
    );
  }
}

export default RootErrorBoundary;
