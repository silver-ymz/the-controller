import { writable, derived } from "svelte/store";
import { command, listen } from "$lib/backend";
import { commands, applyOverrides, type CommandDef } from "$lib/commands";
import { showToast } from "$lib/toast";

interface KeybindingsResult {
  overrides: Record<string, string>;
  warnings: string[];
}

export const keybindingOverrides = writable<Record<string, string>>({});

export const resolvedCommands = derived(keybindingOverrides, ($overrides) =>
  applyOverrides(commands, $overrides),
);

export async function initKeybindings() {
  try {
    const result = await command<KeybindingsResult>("load_keybindings");
    keybindingOverrides.set(result.overrides);
    for (const w of result.warnings) {
      showToast(w, "error");
    }
  } catch {
    // Silently use defaults if backend call fails
  }

  listen<string>("keybindings-changed", (raw) => {
    try {
      const result: KeybindingsResult = JSON.parse(raw);
      keybindingOverrides.set(result.overrides);
      for (const w of result.warnings) {
        showToast(w, "error");
      }
    } catch {
      // Keep current bindings on parse error
    }
  });
}
