// The app (DESIGN.md, Information Architecture): sign in → the account's
// Hosts → the Host shell → a session, with pairing as a full-screen modal. A remembered,
// paired Host opens straight into its shell on launch. In development builds
// only, a `zwiim://dev?provision=…` link (or EXPO_PUBLIC_ZWIIM_DEV_LINK) opens
// the dev screen instead; release builds leave the dev screen out.

import { useCallback, useEffect, useMemo, useState } from "react";
import { AppState, Linking, Platform, useColorScheme } from "react-native";
import { StatusBar } from "expo-status-bar";
import * as SplashScreen from "expo-splash-screen";
import { useFonts } from "expo-font";
import { SafeAreaProvider } from "react-native-safe-area-context";
import { DarkTheme, DefaultTheme, NavigationContainer, type InitialState } from "@react-navigation/native";
import { createNativeStackNavigator, type NativeStackScreenProps } from "@react-navigation/native-stack";
import { Geist_400Regular } from "@expo-google-fonts/geist/400Regular";
import { Geist_500Medium } from "@expo-google-fonts/geist/500Medium";
import { Geist_600SemiBold } from "@expo-google-fonts/geist/600SemiBold";
import { GeistMono_400Regular } from "@expo-google-fonts/geist-mono/400Regular";
import { GeistMono_500Medium } from "@expo-google-fonts/geist-mono/500Medium";
import type { Account } from "./src/account/auth";
import { AccountScope, useAccount, type AccountContext } from "./src/app/account";
import type { RootStack } from "./src/app/navigation";
import { preferences, secureStore } from "./src/platform/native";
import { BrowseMacScreen } from "./src/screens/BrowseMacScreen";
import { ChangesScreen, DiffScreen, FileScreen } from "./src/screens/FilesScreens";
import { HostShell } from "./src/screens/HostShell";
import { HostsScreen } from "./src/screens/HostsScreen";
import { PairingScreen } from "./src/screens/PairingScreen";
import { PreviewScreen } from "./src/screens/PreviewScreen";
import { NewSessionScreen } from "./src/screens/NewSessionScreen";
import { SessionScreen } from "./src/screens/SessionScreen";
import { SignInScreen } from "./src/screens/SignInScreen";
import { TerminalScreenView } from "./src/screens/TerminalScreens";
import { auth, currentHost } from "./src/services";
import { CONTENT_MAX_WIDTH, useTheme } from "./src/ui/tokens";

void SplashScreen.preventAutoHideAsync();

const Stack = createNativeStackNavigator<RootStack>();

// DEV ONLY. `__DEV__` is false in release bundles, so Metro drops this require
// and everything under src/dev with it.
/** On iPad the phone layout sits centered at its own width (DESIGN.md, Layout); the camera stays full-bleed. */
const PAD_CONTENT = Platform.OS === "ios" && Platform.isPad
  ? { width: "100%", maxWidth: CONTENT_MAX_WIDTH, alignSelf: "center" } as const
  : undefined;

const dev: typeof import("./src/dev/DevScreen") | null = __DEV__ ? require("./src/dev/DevScreen") : null;

const isDevLink = (link: string | null | undefined) => !!dev && !!link && dev.devProvisionUrl(link) !== null;

/** The signed-in account, and the host it last opened if this phone is still paired with it. */
async function launch(): Promise<{ account: Account | null; hostOpen: boolean }> {
  const account = await (auth?.current() ?? Promise.resolve(null)).catch(() => null);
  if (!account) return { account, hostOpen: false };
  const last = await preferences.lastHost();
  const pinned = last && (await secureStore.pinnedHost(last.host_id));
  if (!last || !pinned) return { account, hostOpen: false };
  void currentHost.select({
    host_id: last.host_id,
    name: last.name,
    static_pubkey: pinned.host_static_pubkey,
    online: false,
    last_seen_at: null,
    paired: true,
  });
  return { account, hostOpen: true };
}

export default function App() {
  const [fontsLoaded] = useFonts({
    Geist_400Regular,
    Geist_500Medium,
    Geist_600SemiBold,
    GeistMono_400Regular,
    GeistMono_500Medium,
  });
  const theme = useTheme();
  const scheme = useColorScheme();
  // undefined while reading the store.
  const [account, setAccount] = useState<Account | null | undefined>(undefined);
  const [initialState, setInitialState] = useState<InitialState | undefined>(undefined);
  const [devMode, setDevMode] = useState<boolean | undefined>(undefined);
  const [devLink, setDevLink] = useState<string | null>(null);

  useEffect(() => {
    void Linking.getInitialURL().then((link) =>
      setDevMode(isDevLink(link) || isDevLink(process.env.EXPO_PUBLIC_ZWIIM_DEV_LINK)),
    );
    // A dev link while the app runs: the dev screen takes it from here.
    const sub = Linking.addEventListener("url", ({ url }) => {
      if (!isDevLink(url)) return;
      setDevLink(url);
      setDevMode(true);
    });
    void launch().then(({ account, hostOpen }) => {
      if (hostOpen) setInitialState({ routes: [{ name: "Hosts" }, { name: "Host" }] });
      setAccount(account);
    });
    // Back in the foreground: reconnect now rather than after the backoff.
    const app = AppState.addEventListener("change", (s) => s === "active" && currentHost.resume());
    return () => {
      sub.remove();
      app.remove();
    };
  }, []);

  const signedOut = useCallback(() => {
    void currentHost.clear();
    setAccount(null);
  }, []);

  // The account's renewal was refused while connecting: it is signed out.
  useEffect(
    () =>
      currentHost.subscribe(() => {
        if (currentHost.state.view?.error?.code !== "unauthenticated") return;
        void auth?.current().then((a) => a || signedOut());
      }),
    [signedOut],
  );

  const scope = useMemo<AccountContext | null>(
    () =>
      account
        ? {
            account,
            signedOut,
            signOut: async () => {
              // Pairings stay: signing in again reconnects without pairing.
              void auth?.signOut();
              signedOut();
            },
          }
        : null,
    [account, signedOut],
  );

  const navTheme = useMemo(() => {
    const base = scheme === "light" ? DefaultTheme : DarkTheme;
    return {
      ...base,
      colors: { ...base.colors, background: theme.bg, card: theme.bg, text: theme.fg, border: theme.line, primary: theme.accent },
    };
  }, [scheme, theme]);

  const ready = fontsLoaded && devMode !== undefined && account !== undefined;
  useEffect(() => {
    if (ready) void SplashScreen.hideAsync();
  }, [ready]);
  if (!ready) return null;

  return (
    <SafeAreaProvider>
      {devMode && dev ? (
        <dev.default link={devLink} />
      ) : (
        <AccountScope.Provider value={scope}>
          <NavigationContainer theme={navTheme} initialState={initialState}>
            <Stack.Navigator screenOptions={{ headerShown: false, contentStyle: PAD_CONTENT }}>
              {!account ? (
                <Stack.Screen name="SignIn">{() => <SignInScreen onSignedIn={setAccount} />}</Stack.Screen>
              ) : (
                <>
                  <Stack.Screen name="Hosts" component={HostsScreen} />
                  <Stack.Screen name="Host" component={HostShell} />
                  <Stack.Screen name="Session" component={SessionScreen} />
                  <Stack.Screen name="NewSession" component={NewSessionScreen} />
                  <Stack.Screen name="Changes" component={ChangesScreen} />
                  <Stack.Screen name="File" component={FileScreen} />
                  <Stack.Screen name="Preview" component={PreviewScreen} />
                  <Stack.Screen name="Diff" component={DiffScreen} />
                  <Stack.Screen name="TerminalView" component={TerminalScreenView} />
                  <Stack.Screen name="BrowseMac" component={BrowseMacScreen} />
                  <Stack.Screen name="Pair" component={PairRoute} options={{ presentation: "fullScreenModal", contentStyle: undefined }} />
                </>
              )}
            </Stack.Navigator>
          </NavigationContainer>
        </AccountScope.Provider>
      )}
      <StatusBar style="auto" />
    </SafeAreaProvider>
  );
}

function PairRoute({ route, navigation }: NativeStackScreenProps<RootStack, "Pair">) {
  const { host, hosts } = route.params;
  const { signOut } = useAccount();
  return (
    <PairingScreen
      host={host ?? null}
      hosts={hosts ?? []}
      onClose={() => navigation.goBack()}
      // Connect at once, while the done screen shows.
      onPaired={(paired) => void currentHost.open({ ...paired, paired: true })}
      onOpen={() => navigation.popTo("Host")}
      onSignIn={() => void signOut()}
    />
  );
}
