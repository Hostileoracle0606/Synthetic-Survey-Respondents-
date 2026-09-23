import type { ReactNode } from "react";
import { Stepper } from "./Stepper";

type Props = { title: string; heading: string; subtitle: string; children: ReactNode; footer?: ReactNode };

/** Page frame from the prototype: header with home button and stepper, one large card. */
export function AppShell({ title, heading, subtitle, children, footer }: Props) {
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
        <Stepper />
      </header>
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
