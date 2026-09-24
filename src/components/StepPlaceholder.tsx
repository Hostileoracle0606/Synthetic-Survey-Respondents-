import { AppShell } from "./AppShell";

type Props = { title: string; heading: string; subtitle: string; milestone: string; items: string[] };

/** Stands in for a step until its milestone lands. Lists what the step will do. */
export function StepPlaceholder({ title, heading, subtitle, milestone, items }: Props) {
  return (
    <AppShell title={title} heading={heading} subtitle={subtitle}>
      <div className="rounded-[22px] border border-dashed border-line bg-white p-8">
        <p className="m-0 font-display text-lg font-medium">Planned for {milestone}</p>
        <ul className="mb-0 mt-3 flex flex-col gap-1.5 pl-5 text-muted">
          {items.map((i) => <li key={i}>{i}</li>)}
        </ul>
      </div>
    </AppShell>
  );
}
