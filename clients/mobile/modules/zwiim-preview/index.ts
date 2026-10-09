// In-app previews: a web view with no bridge to the app. HTML pages get
// their own origin whose requests the app answers; other files load from
// the app cache (iOS: WebKit renders them; Android: the bundled renderer).

import { requireNativeModule, requireNativeView } from "expo";
import type { StyleProp, ViewStyle } from "react-native";

export type PreviewSource =
  | { kind: "html"; previewId: string; startPath: string }
  | { kind: "file"; previewId: string; file: string; render: "native" | "renderer"; name: string; mime_type: string; type: string };

export interface PreviewResponse {
  status: number;
  mime_type?: string;
  file?: string;
  location?: string;
  text?: string;
}

export interface ZwiimPreviewViewProps {
  source: PreviewSource;
  onRequest?: (e: { nativeEvent: { requestId: string; method: string; path: string } }) => void;
  onFailed?: (e: { nativeEvent: { message: string } }) => void;
  style?: StyleProp<ViewStyle>;
}

const native = requireNativeModule<{ respondPreview(requestId: string, response: PreviewResponse): Promise<void> }>("ZwiimPreview");

export const ZwiimPreviewView = requireNativeView<ZwiimPreviewViewProps>("ZwiimPreview");

export function respondPreview(requestId: string, response: PreviewResponse): Promise<void> {
  return native.respondPreview(requestId, response);
}
