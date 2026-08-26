import { Component, type ErrorInfo, type ReactNode } from "react";

type Props = {
  children: ReactNode;
};

type State = {
  error: Error | null;
};

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("UI crashed:", error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="flex h-full items-center justify-center bg-[#0f1012] px-6 text-[#f7f8f8]">
          <div className="max-w-md rounded-xl border border-[#2a2c31] bg-[#141516] p-5">
            <h1 className="m-0 mb-2 text-lg font-semibold">Something broke</h1>
            <p className="m-0 mb-4 text-sm text-[#8a8f98]">
              Zest could not display this screen. Try again or restart the app.
            </p>
            <button
              type="button"
              className="rounded-md bg-[#5e6ad2] px-3 py-1.5 text-sm font-medium text-white"
              onClick={() => this.setState({ error: null })}
            >
              Try again
            </button>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
