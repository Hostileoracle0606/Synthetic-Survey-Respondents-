import { useState, type ReactNode } from "react";
import { SettingsScreen } from "./SettingsScreen";
import { Stepper } from "./Stepper";

type Props = { title: string; heading: string; subtitle: string; children: ReactNode; footer?: ReactNode };

/** Page frame from the prototype: header with home button and stepper, one large card. */
export function AppShell({ title, heading, subtitle, children, footer }: Props) {
  const [showSettings, setShowSettings] = useState(false);
  return (
    <div className="flex min-h-screen flex-col gap-7 px-10 pb-10 pt-8">
      <header className="flex items-center justify-between gap-6">
        <div className="flex items-center gap-5">
          <span aria-hidden className="flex h-[60px] w-[60px] items-center justify-center rounded-full bg-olive text-white">
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
              <path d="M3 10.5 12 3l9 7.5V20a1 1 0 0 1-1 1h-5v-6h-6v6H4a1 1 0 0 1-1-1z" />
            </svg>
          </span>
          <span className="font-display text-[34px] tracking-tight text-[#3A3A36]">{title}</span>
        </div>
        <div className="flex items-center gap-4">
          <Stepper />
          <button
            type="button"
            aria-label="Settings"
            onClick={() => setShowSettings(true)}
            className="flex h-11 w-11 items-center justify-center rounded-full border border-line hover:bg-[#f1f1ee]"
          >
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1.04 1.56V21a2 2 0 0 1-4 0v-.09A1.7 1.7 0 0 0 9 19.35a1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.87 1.7 1.7 0 0 0-1.56-1.04H3a2 2 0 0 1 0-4h.09A1.7 1.7 0 0 0 4.65 9a1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34H9a1.7 1.7 0 0 0 1.04-1.56V3a2 2 0 0 1 4 0v.09a1.7 1.7 0 0 0 1.04 1.56 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87V9a1.7 1.7 0 0 0 1.56 1.04H21a2 2 0 0 1 0 4h-.09a1.7 1.7 0 0 0-1.56 1.04z" />
            </svg>
          </button>
        </div>
      </header>
      {showSettings && <SettingsScreen onClose={() => setShowSettings(false)} />}
      <main className="flex flex-1 flex-col gap-6 rounded-[44px] bg-card px-16 py-11 shadow-sm">
        <div className="flex flex-col gap-2 border-b border-divider pb-5">
          <h1 className="m-0 font-display text-3xl font-semibold">{heading}</h1>
          <p className="m-0 text-[17px] text-muted">{subtitle}</p>
        </div>
        {children}
        {footer && <div className="mt-auto flex items-center justify-between">{footer}</div>}
      </main>
    </div>
  );
}
