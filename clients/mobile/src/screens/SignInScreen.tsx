// Sign in (DESIGN.md, Screen 1): "Continue with Google" through the account.

import { useState } from "react";
import { StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { SignInCancelled, type Account } from "../account/auth";
import { auth } from "../services";
import { Button, T } from "../ui/components";
import { space, useTheme } from "../ui/tokens";

export function SignInScreen({ onSignedIn }: { onSignedIn(account: Account): void }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);

  const signIn = async () => {
    if (!auth) return;
    setPending(true);
    setFailed(false);
    try {
      onSignedIn(await auth.signIn());
    } catch (e) {
      // Cancelled: back to the button, silently.
      if (!(e instanceof SignInCancelled)) setFailed(true);
    } finally {
      setPending(false);
    }
  };

  return (
    <View style={[styles.page, { backgroundColor: theme.bg, paddingTop: insets.top + space.xxxl, paddingBottom: insets.bottom + space.xl }]}>
      <View style={styles.top}>
        <T variant="display">Zwiim</T>
        <T variant="headline" color={theme.fg}>
          Control your Zwiim Hosts from anywhere.
        </T>
        <View style={styles.lines}>
          <T color={theme.mut}>See your Hosts and whether they're online.</T>
          <T color={theme.mut}>Connect from any network.</T>
          <T color={theme.mut}>Pair this phone securely.</T>
        </View>
      </View>
      <View style={styles.bottom}>
        <Button
          testID="sign-in"
          kind="inverse"
          tall
          label="Continue with Google"
          onPress={() => void signIn()}
          loading={pending}
          disabled={!auth}
        />
        {!auth ? (
          <T variant="callout" color={theme.err}>
            Sign-in isn't set up in this build.
          </T>
        ) : failed ? (
          <T variant="callout" color={theme.err}>
            Couldn't sign in. Try again.
          </T>
        ) : null}
        <T variant="callout" color={theme.mut}>
          Your code stays on your Mac.
        </T>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1, paddingHorizontal: space.lg, justifyContent: "space-between" },
  top: { gap: space.md },
  lines: { marginTop: space.lg, gap: space.xs },
  bottom: { gap: space.md },
});
