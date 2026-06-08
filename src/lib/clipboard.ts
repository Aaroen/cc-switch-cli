<<<<<<< HEAD
import { invoke } from "@/lib/api/transport";
=======
import { invoke } from "@tauri-apps/api/core";
>>>>>>> origin/cc-switch-cli

export async function copyText(text: string): Promise<void> {
  try {
    await invoke("copy_text_to_clipboard", { text });
    return;
  } catch (nativeError) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch (webError) {
      throw webError instanceof Error
        ? webError
        : nativeError instanceof Error
          ? nativeError
          : new Error(String(webError || nativeError));
    }
  }
}
