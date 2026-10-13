import { useState } from "react";
import type { CueError } from "../api";

const TITLES: Partial<Record<CueError["kind"], string>> = {
  auth: "Authentication problem",
  permission_denied: "Access denied",
  model_not_found: "Model not available",
  rate_limited: "Rate limit",
  quota_exceeded: "No credit or quota left",
  subscription_limit: "Plan limit reached",
  overloaded: "Provider busy",
  network: "Connection problem",
  timeout: "Timed out",
  not_configured: "Not set up",
  context_too_long: "Conversation too long",
  refusal: "Declined",
  secret_store: "Credential store problem",
  invalid_request: "Please check",
};

export function ErrorCard({
  error,
  onRetry,
  onOpenProviders,
}: {
  error: CueError;
  onRetry?: () => void;
  onOpenProviders?: () => void;
}) {
  const [open, setOpen] = useState(false);
  if (error.kind === "cancelled") return <div className="notice subtle">Stopped.</div>;
  const providerIssue = ["auth", "permission_denied", "model_not_found", "not_configured", "quota_exceeded"].includes(error.kind);
  return (
    <div className="error-card" role="alert">
      <div className="error-title">{TITLES[error.kind] ?? "Something went wrong"}</div>
      <div>{error.message}</div>
      {error.hint && <div className="error-hint">{error.hint}</div>}
      <div className="error-actions">
        {error.retryable && onRetry && (
          <button className="small" onClick={onRetry}>
            Retry
          </button>
        )}
        {providerIssue && onOpenProviders && (
          <button className="small" onClick={onOpenProviders}>
            Open AI Providers
          </button>
        )}
        {(error.detail || error.status || error.request_id) && (
          <button className="ghost small" onClick={() => setOpen((o) => !o)}>
            {open ? "Hide details" : "Details"}
          </button>
        )}
      </div>
      {open && (
        <pre className="error-detail">
          {[
            error.status ? `HTTP ${error.status}` : null,
            error.detail,
            error.request_id ? `Request ID: ${error.request_id}` : null,
          ]
            .filter(Boolean)
            .join("\n")}
        </pre>
      )}
    </div>
  );
}
