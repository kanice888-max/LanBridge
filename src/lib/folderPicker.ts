import { open } from "@tauri-apps/api/dialog";

export async function pickFolder(title: string): Promise<string | null> {
  const selected = await open({
    directory: true,
    multiple: false,
    title,
  });

  if (Array.isArray(selected)) {
    return selected[0] ?? null;
  }

  return selected;
}
