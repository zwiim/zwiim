/** Pasted text longer than this goes with the message as a file, not into the box. */
export const LONG_PASTE_CHARS = 5000;

/** The name a long paste is attached under. */
export const LONG_PASTE_NAME = "Pasted text.txt";

/**
 * What a paste into a composer attaches: its files, else its text as one file
 * when `convertLongText` and the text is over `LONG_PASTE_CHARS`. Null leaves
 * the paste to the text box.
 */
export function pastedFiles(data: DataTransfer, convertLongText: boolean): File[] | null {
  if (data.files.length > 0) return [...data.files];
  if (!convertLongText) return null;
  const text = data.getData("text/plain");
  if (text.length <= LONG_PASTE_CHARS) return null;
  return [new File([text], LONG_PASTE_NAME, { type: "text/plain" })];
}
