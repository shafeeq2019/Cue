import { useEffect, useState } from "react";

/** Two-step destructive action: first click arms, second click confirms. */
export function ConfirmButton({
  label,
  confirmLabel = "Click again to confirm",
  onConfirm,
  className = "small danger",
  disabled,
}: {
  label: string;
  confirmLabel?: string;
  onConfirm: () => void | Promise<void>;
  className?: string;
  disabled?: boolean;
}) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 4000);
    return () => clearTimeout(t);
  }, [armed]);
  return (
    <button
      className={`${className} ${armed ? "armed" : ""}`}
      disabled={disabled}
      onBlur={() => setArmed(false)}
      onClick={async () => {
        if (!armed) return setArmed(true);
        setArmed(false);
        await onConfirm();
      }}
    >
      {armed ? confirmLabel : label}
    </button>
  );
}
