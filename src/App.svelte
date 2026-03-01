<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import Sidebar from "./lib/Sidebar.svelte";
  import TerminalManager from "./lib/TerminalManager.svelte";
  import Onboarding from "./lib/Onboarding.svelte";
  import Toast from "./lib/Toast.svelte";
  import HotkeyManager from "./lib/HotkeyManager.svelte";
  import HotkeyHelp from "./lib/HotkeyHelp.svelte";
  import StatusBar from "./lib/StatusBar.svelte";
  import { appConfig, onboardingComplete, hotkeyAction, type Config } from "./lib/stores";

  let ready = $state(false);
  let needsOnboarding = $state(true);
  let showHelp = $state(false);

  hotkeyAction.subscribe((action) => {
    if (action?.type === "toggle-help") {
      showHelp = !showHelp;
    }
  });

  onMount(async () => {
    try {
      const config = await invoke<Config | null>("check_onboarding");
      if (config) {
        appConfig.set(config);
        onboardingComplete.set(true);
        needsOnboarding = false;
      }
    } catch (e) {
      // Config check failed, show onboarding
    }
    ready = true;
  });

  // Listen for onboarding completion
  onboardingComplete.subscribe((complete) => {
    if (complete) needsOnboarding = false;
  });
</script>

{#if ready}
  {#if needsOnboarding}
    <Onboarding />
  {:else}
    <div class="app-layout">
      <Sidebar />
      <main class="terminal-area">
        <TerminalManager />
      </main>
    </div>
    <HotkeyManager />
    <StatusBar />
    {#if showHelp}
      <HotkeyHelp onClose={() => showHelp = false} />
    {/if}
  {/if}
{/if}
<Toast />

<style>
  .app-layout {
    display: flex;
    height: 100vh;
    width: 100vw;
    background: #11111b;
    overflow: hidden;
  }
  .terminal-area {
    flex: 1;
    overflow: hidden;
  }
</style>
