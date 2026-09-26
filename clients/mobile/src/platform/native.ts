// The phone's implementations of the capabilities, over native modules.

import { randomUUID } from "expo-crypto";
import * as SecureStoreModule from "expo-secure-store";
import ZwiimLan from "../../modules/zwiim-lan";
import { nativeDial } from "./lan";
import { VaultPreferences, VaultSecureStore, type Vault } from "./vault-store";

/** TCP connections to hosts on the local network. */
export const lanDial = nativeDial(ZwiimLan, randomUUID);

// Readable once the phone has been unlocked after a restart, and never
// migrated to another device by a backup.
const options = { keychainAccessible: SecureStoreModule.AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY };

const vault: Vault = {
  getItemAsync: (key) => SecureStoreModule.getItemAsync(key, options),
  setItemAsync: (key, value) => SecureStoreModule.setItemAsync(key, value, options),
  deleteItemAsync: (key) => SecureStoreModule.deleteItemAsync(key, options),
};

/** The device key, pinned host keys and account session: Keychain / Keystore. */
export const secureStore = new VaultSecureStore(vault);

/** Connection modes and the last host, beside them. */
export const preferences = new VaultPreferences(vault);
