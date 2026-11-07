// Modified from upstream Apache-2.0 source; see NOTICE.
// Adapted from the upstream web UI's `src/components/ui/button.tsx` (shadcn variant set; see NOTICE).
import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
  // Named transitions only (DESIGN.md "Motion"); a press scales to .98; a disabled button keeps
  // its pointer events so its tooltip still explains why.
  "inline-flex shrink-0 items-center justify-center gap-1.5 rounded-md border border-transparent text-sm font-medium whitespace-nowrap transition-[background-color,color,border-color,box-shadow,opacity,transform] duration-120 ease-std select-none enabled:active:scale-[.98] enabled:active:duration-75 disabled:cursor-not-allowed disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground enabled:hover:bg-primary/85",
        brand: "bg-brand text-brand-foreground enabled:hover:bg-brand/88",
        done: "bg-done text-done-foreground enabled:hover:bg-done/88",
        outline: "border-border bg-input/30 enabled:hover:bg-input/60",
        secondary: "bg-secondary text-secondary-foreground enabled:hover:bg-accent",
        ghost: "enabled:hover:bg-accent enabled:hover:text-foreground",
        destructive:
          "bg-destructive/10 text-destructive enabled:hover:bg-destructive/20 dark:bg-destructive/15 dark:enabled:hover:bg-destructive/25",
      },
      size: {
        default: "h-9 px-3",
        xs: "h-6 gap-1 px-2 text-xs [&_svg:not([class*='size-'])]:size-3",
        sm: "h-8 px-3",
        icon: "size-9",
        "icon-sm": "size-7",
      },
    },
    defaultVariants: { variant: "default", size: "default" },
  },
);

export function Button({
  className,
  variant,
  size,
  ...props
}: React.ComponentProps<"button"> & VariantProps<typeof buttonVariants>) {
  return <button className={cn(buttonVariants({ variant, size, className }))} {...props} />;
}
