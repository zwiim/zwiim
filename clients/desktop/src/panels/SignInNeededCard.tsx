import { Button } from "@/components/ui/button";
import { useOpenSettings } from "@/settings/open";

/**
 * The agent refused the turn until it is signed in again. Signing in happens
 * in settings, so the card opens them at the agents; where no settings are
 * offered it only says why the turn stopped.
 */
export function SignInNeededCard({ message }: { message: string }) {
  const openSettings = useOpenSettings();
  return (
    <div
      // `surface` card with the 3pt `err` rule on its leading edge.
      className="bg-card animate-enter flex flex-col gap-2 rounded-xl p-4 shadow-[inset_3px_0_0_var(--destructive)] motion-reduce:animate-none"
      data-testid="sign-in-needed-card"
      role="alert"
    >
      <p className="text-sm">{message}</p>
      {openSettings && (
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="ghost"
            className="text-muted-foreground"
            onClick={() => openSettings({ tab: "agents" })}
          >
            Open settings →
          </Button>
        </div>
      )}
    </div>
  );
}
