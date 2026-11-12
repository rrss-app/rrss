import { Component, type ReactNode } from "react";
import { reportClientError } from "../lib/clientLog";

type Props = {
  scope?: string;
  /** 变化时重置错误状态，例如换到另一篇文章。 */
  resetKey?: unknown;
  fallback: (error: unknown, reset: () => void) => ReactNode;
  onError?: (error: unknown) => void;
  children: ReactNode;
};

type State = { error: unknown; failed: boolean };

/**
 * 局部错误边界：只隔离子树的渲染异常，外层的返回 / 关闭仍可用。
 * 事件处理器与异步代码里的错误不经过这里，仍由调用方自己处理。
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, failed: false };

  static getDerivedStateFromError(error: unknown): State {
    return { error, failed: true };
  }

  componentDidCatch(error: unknown) {
    if (this.props.onError) this.props.onError(error);
    else reportClientError(this.props.scope ?? "render", error);
  }

  componentDidUpdate(prev: Props) {
    if (this.state.failed && prev.resetKey !== this.props.resetKey) this.reset();
  }

  reset = () => {
    this.setState({ error: null, failed: false });
  };

  render() {
    if (this.state.failed) return this.props.fallback(this.state.error, this.reset);
    return this.props.children;
  }
}
