// The sheet at a relay limit (browser design 15.6, DESIGN.md Global States):
// what stopped, when it resets, and what still works. Shown once each time
// the limit is hit, with the Cloud's usage when it answers.

import type { CloudUsage } from "@zwiim/sdk";
import { useEffect, useState } from "react";
import { StyleSheet, View } from "react-native";
import { limitSheet, type RelayLimit } from "../account/usage";
import { cloud } from "../services";
import { Button, Sheet, T } from "../ui/components";
import { space, useTheme } from "../ui/tokens";

export function LimitSheet({ limit, hit, hostName }: { limit: RelayLimit; hit: boolean; hostName: string }) {
  const theme = useTheme();
  const [shown, setShown] = useState<{ usage: CloudUsage | null } | null>(null);
  useEffect(() => {
    if (!hit) return;
    let gone = false;
    const show = (usage: CloudUsage | null) => {
      if (!gone) setShown({ usage });
    };
    if (cloud) cloud.usage().then(show, () => show(null));
    else show(null);
    return () => {
      gone = true;
    };
  }, [hit]);

  const sheet = shown ? limitSheet(limit, shown.usage, hostName) : null;
  return (
    <Sheet visible={sheet !== null} onClose={() => setShown(null)}>
      {sheet ? (
        <View style={styles.body}>
          <T variant="title">{sheet.title}</T>
          {sheet.lines.map((line) => (
            <T key={line} color={theme.mut}>
              {line}
            </T>
          ))}
          <Button tall label="OK" onPress={() => setShown(null)} />
        </View>
      ) : null}
    </Sheet>
  );
}

const styles = StyleSheet.create({
  body: { gap: space.sm },
});
