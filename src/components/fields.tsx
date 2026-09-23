import type { ReactNode } from "react";

export const pillField =
  "h-14 w-full rounded-full border border-line bg-white px-6 text-base text-ink placeholder:text-[#76766f] focus:border-accent focus:outline-none focus:ring-4 focus:ring-accent/15";
export const areaField =
  "w-full resize-none rounded-[26px] border border-line bg-white px-6 py-4 text-[15px] leading-relaxed placeholder:text-[#76766f] focus:border-accent focus:outline-none focus:ring-4 focus:ring-accent/15";
export const pillButton =
  "inline-flex h-[52px] items-center justify-center gap-2.5 rounded-full border border-ink px-6 font-display text-base font-medium hover:bg-[#f1f1ee] disabled:cursor-not-allowed disabled:opacity-40";
export const primaryButton =
  "inline-flex h-[52px] items-center justify-center gap-2.5 rounded-full bg-accent px-6 font-display text-base font-medium text-white hover:bg-accent-dark disabled:cursor-not-allowed disabled:opacity-40";

export function Label({ htmlFor, id, children }: { htmlFor?: string; id?: string; children: ReactNode }) {
  return (
    <label htmlFor={htmlFor} id={id} className="font-display text-[15px] font-medium">
      {children}
    </label>
  );
}

export function Help({ children }: { children: ReactNode }) {
  return <span className="text-sm leading-snug text-muted">{children}</span>;
}

export function Arrow() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M5 12h14M13 6l6 6-6 6" />
    </svg>
  );
}
