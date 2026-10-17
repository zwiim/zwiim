import { cn } from "@/lib/utils";

/** Each cell's step on the snake's clockwise walk round the 3×3 border; the center sits out. */
const WALK = [0, 1, 2, 7, null, 3, 6, 5, 4];

/** An agent at work: a short snake of dots walking the edge of a 3×3 grid (still under Reduce Motion). */
export function WorkingMark({ className }: { className?: string }) {
  return (
    <span className={cn("grid size-3.5 shrink-0 place-items-center", className)} aria-hidden>
      <span className="grid grid-cols-[repeat(3,3px)] grid-rows-[repeat(3,3px)] gap-[1.5px]">
        {WALK.map((step, i) => (
          <span
            key={i}
            className={cn(
              "bg-muted-foreground size-[3px] rounded-full",
              step === null ? "opacity-[.12]" : "animate-snake-dot opacity-[.12] motion-reduce:animate-none",
            )}
            style={step === null ? undefined : { animationDelay: `${step * 150}ms` }}
          />
        ))}
      </span>
    </span>
  );
}
