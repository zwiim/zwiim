// Picking files on the phone, reading them to upload, and handing a
// workspace file to another app through the share sheet.

import { randomUUID } from "expo-crypto";
import * as DocumentPicker from "expo-document-picker";
import { Directory, File, Paths } from "expo-file-system";
import * as ImagePicker from "expo-image-picker";
import * as Sharing from "expo-sharing";
import type { FileDownload } from "@zwiim/sdk";
import { fileNameOf, type PickedFile } from "./pending";

function picked(uri: string, name: string | null | undefined, size: number | null | undefined, mime: string | null | undefined): PickedFile {
  return {
    uri,
    name: fileNameOf(uri, name),
    size: size ?? new File(uri).size ?? 0,
    mime_type: mime || "application/octet-stream",
  };
}

/** Photos from the library; none when cancelled. */
export async function pickPhotos(): Promise<PickedFile[]> {
  const result = await ImagePicker.launchImageLibraryAsync({ mediaTypes: ["images"], allowsMultipleSelection: true, quality: 1 });
  if (result.canceled) return [];
  return result.assets.map((a) => picked(a.uri, a.fileName, a.fileSize, a.mimeType));
}

/** Any files from Files / the document picker; none when cancelled. */
export async function pickFiles(): Promise<PickedFile[]> {
  const result = await DocumentPicker.getDocumentAsync({ multiple: true, copyToCacheDirectory: true });
  if (result.canceled) return [];
  return result.assets.map((a) => picked(a.uri, a.name, a.size, a.mimeType));
}

/** Standard base64 of a picked file's bytes. */
export function readBase64(uri: string): Promise<string> {
  return new File(uri).base64();
}

/** Keep a downloaded workspace file in the cache and offer it to other apps (the iOS sheet previews it). */
export async function shareDownload(file: FileDownload): Promise<void> {
  const dir = new Directory(Paths.cache, "zwiim-downloads", randomUUID());
  dir.create({ intermediates: true, idempotent: true });
  const copy = new File(dir, file.name);
  copy.write(file.data, { encoding: "base64" });
  await Sharing.shareAsync(copy.uri, { mimeType: file.mime_type, dialogTitle: file.name });
}
